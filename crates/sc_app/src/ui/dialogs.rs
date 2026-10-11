//! Model Info, Preferences (shortcuts), About, save-changes prompt and error messages.

use crate::app::SketchCadApp;
use sc_math::units::LengthUnit;

impl SketchCadApp {
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        self.model_info(ctx);
        self.component_dialog(ctx);
        self.preferences(ctx);
        self.about(ctx);
        self.save_prompt(ctx);
        self.error(ctx);
    }

    fn component_dialog(&mut self, ctx: &egui::Context) {
        let Some((mut name, mut desc)) = self.component_dialog.take() else { return };
        let mut done = None;
        let modal = egui::Modal::new(egui::Id::new("make_component")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Create Component");
            egui::Grid::new("mc").num_columns(2).show(ui, |ui| {
                ui.label("Definition");
                ui.text_edit_singleline(&mut name).request_focus();
                ui.end_row();
                ui.label("Description");
                ui.text_edit_singleline(&mut desc);
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) if !name.trim().is_empty() => self.make_definition(false, Some(name.trim().to_string()), desc),
            Some(_) => {}
            None if modal.should_close() => {}
            None => self.component_dialog = Some((name, desc)),
        }
    }

    fn model_info(&mut self, ctx: &egui::Context) {
        let mut open = self.panels.model_info;
        egui::Window::new("Model Info").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.heading("Units");
            let u = &mut self.doc.model.units;
            egui::Grid::new("units").num_columns(2).show(ui, |ui| {
                ui.label("Length");
                egui::ComboBox::from_id_salt("unit").selected_text(u.unit.label()).show_ui(ui, |ui| {
                    for unit in LengthUnit::ALL {
                        ui.selectable_value(&mut u.unit, unit, unit.label());
                    }
                });
                ui.end_row();
                if u.unit == LengthUnit::Architectural {
                    ui.label("Precision");
                    egui::ComboBox::from_id_salt("frac").selected_text(format!("1/{}\"", u.fraction)).show_ui(ui, |ui| {
                        for d in [1, 2, 4, 8, 16, 32, 64] {
                            ui.selectable_value(&mut u.fraction, d, format!("1/{d}\""));
                        }
                    });
                } else {
                    ui.label("Precision");
                    ui.add(egui::Slider::new(&mut u.precision, 0..=6).text("decimals"));
                }
                ui.end_row();
            });
            ui.separator();
            ui.heading("Statistics");
            let m = &self.doc.model.mesh;
            egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
                for (k, v) in [("Vertices", m.vertices.len()), ("Edges", m.edges.len()), ("Faces", m.faces.len()), ("Guides", self.doc.model.guides.len())] {
                    ui.label(k);
                    ui.label(v.to_string());
                    ui.end_row();
                }
            });
            if let Err(e) = m.validate() {
                ui.colored_label(egui::Color32::RED, format!("Problem found: {e}"));
            }
        });
        self.panels.model_info = open;
    }

    fn preferences(&mut self, ctx: &egui::Context) {
        let mut open = self.panels.preferences;
        egui::Window::new("Preferences — Shortcuts").open(&mut open).default_width(360.0).show(ctx, |ui| {
            ui.label("SketchCAD uses SketchUp's default keyboard shortcuts.");
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("shortcuts").striped(true).num_columns(2).show(ui, |ui| {
                    for (s, c) in &self.keymap.bindings {
                        ui.label(c.label());
                        ui.monospace(ctx.format_shortcut(s));
                        ui.end_row();
                    }
                    for (what, how) in [
                        ("Orbit", "Middle mouse drag"),
                        ("Pan", "Shift + middle mouse drag"),
                        ("Zoom", "Scroll wheel"),
                        ("Lock Blue / Red / Green axis", "Up / Right / Left arrow"),
                        ("Lock current inference", "Hold Shift"),
                        ("Copy / new face toggle", if cfg!(target_os = "macos") { "Option" } else { "Ctrl" }),
                        ("Cancel", "Esc"),
                    ] {
                        ui.label(what);
                        ui.monospace(how);
                        ui.end_row();
                    }
                });
            });
        });
        self.panels.preferences = open;
    }

    fn about(&mut self, ctx: &egui::Context) {
        let mut open = self.panels.about;
        egui::Window::new("About SketchCAD").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.heading("SketchCAD");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("A fast, intuitive 3D modeler written in Rust.");
        });
        self.panels.about = open;
    }

    fn save_prompt(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending else { return };
        let name = self.doc.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let modal = egui::Modal::new(egui::Id::new("save_prompt")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading("SketchCAD");
            ui.label(format!("Save changes to \"{name}\" before closing?"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Yes").clicked()
                    && self.save(false) {
                        self.pending = None;
                        self.perform(action);
                    }
                if ui.button("No").clicked() {
                    self.pending = None;
                    self.doc.dirty = false;
                    self.perform(action);
                }
                if ui.button("Cancel").clicked() {
                    self.pending = None;
                }
            });
        });
        if modal.should_close() {
            self.pending = None;
        }
    }

    fn error(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.error_dialog.clone() else { return };
        let modal = egui::Modal::new(egui::Id::new("error")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("SketchCAD");
            ui.label(msg);
            if ui.button("OK").clicked() {
                self.error_dialog = None;
            }
        });
        if modal.should_close() {
            self.error_dialog = None;
        }
    }
}
