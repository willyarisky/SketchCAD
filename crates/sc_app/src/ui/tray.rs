//! The Default Tray: Entity Info, Materials, Styles, Instructor.

use super::format_area;
use super::icons::{Icon, paint};
use crate::app::SketchCadApp;
use crate::tools::ToolId;
use egui::{Color32, CollapsingHeader, Frame, Margin, RichText};
use crate::materials::MATERIALS;
use sc_geom::Color;
use sc_render::FaceStyle;

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

    /// The tile shown on a swatch, rendered and uploaded once per session.
    fn material_preview(&mut self, ctx: &egui::Context, name: &'static str, p: crate::materials::Pattern, color: Color) -> egui::TextureId {
        if let Some(h) = self.material_previews.get(name) {
            return h.id();
        }
        let img = crate::materials::render(p, color);
        let side = crate::materials::TILE_PX as usize;
        let h = ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([side, side], img.as_raw()), egui::TextureOptions::LINEAR);
        let id = h.id();
        self.material_previews.insert(name, h);
        id
    }

    /// Choose a palette material, putting its tile into the model the first time it is used.
    pub fn pick_material(&mut self, def: &'static crate::materials::MaterialDef) {
        self.material = def.color;
        self.material_tex = def.pattern.map(|p| {
            // One copy per material: painting the same brick twice must not store it twice.
            let existing = self.doc.model.textures.iter().find(|(_, t)| t.name == def.name).map(|(id, _)| id);
            let id = existing.unwrap_or_else(|| {
                self.doc.model.textures.insert(sc_geom::Texture {
                    name: def.name.to_string(),
                    width: crate::materials::TILE_PX,
                    height: crate::materials::TILE_PX,
                    size: [def.tile, def.tile],
                    data: crate::materials::render_png(p, def.color),
                })
            });
            (id, def.tile)
        });
    }

    fn materials(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            swatch(ui, self.material);
            let name = MATERIALS.iter().find(|m| m.color == self.material).map_or("Custom", |m| m.name);
            ui.label(name);
        });
        ui.add_space(4.0);
        let per_row = 6;
        let mut chosen: Option<&'static crate::materials::MaterialDef> = None;
        egui::Grid::new("materials").spacing([4.0, 4.0]).show(ui, |ui| {
            for (i, def) in MATERIALS.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                match def.pattern {
                    // Show the real tile, so the swatch is what you will actually get.
                    Some(p) => {
                        let tex = self.material_preview(ui.ctx(), def.name, p, def.color);
                        ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().rect_filled(rect, 3.0, c32(def.color));
                    }
                }
                if def.color == self.material {
                    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, Color32::from_rgb(30, 90, 200)), egui::StrokeKind::Outside);
                }
                if resp.on_hover_text(def.name).clicked() {
                    chosen = Some(def);
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        if let Some(def) = chosen {
            self.pick_material(def);
            self.set_tool(ToolId::PaintBucket);
        }
        ui.horizontal(|ui| {
            ui.label("Custom");
            let mut rgba = egui::Rgba::from(c32(self.material));
            if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::OnlyBlend).changed() {
                let c = Color32::from(rgba);
                self.material = [c.r(), c.g(), c.b(), c.a()];
                // A hand-picked colour is a plain colour; drop any pattern that was selected.
                self.material_tex = None;
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
