//! Tray panels for groups and components: instance Entity Info, Components, Tags, Outliner.

use crate::app::SketchCadApp;
use crate::document::Tag;
use egui::{Color32, RichText};
use sc_geom::{DefId, InstanceId};

const TAG_COLORS: [[u8; 4]; 8] = [
    [220, 90, 80, 255],
    [80, 150, 220, 255],
    [90, 180, 90, 255],
    [230, 170, 50, 255],
    [150, 100, 200, 255],
    [60, 170, 170, 255],
    [200, 110, 160, 255],
    [130, 130, 130, 255],
];

fn tag_label(tags: &[Tag], t: Option<u32>) -> String {
    t.and_then(|i| tags.get(i as usize)).map_or_else(|| "Untagged".to_string(), |t| t.name.clone())
}

/// Outliner icon: a square for groups, a square with a diamond for components.
fn outliner_icon(ui: &mut egui::Ui, is_group: bool) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let r = r.shrink(1.5);
    let stroke = egui::Stroke::new(1.2, Color32::from_gray(60));
    ui.painter().rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
    if !is_group {
        let c = r.center();
        let d = r.width() * 0.32;
        let pts = vec![c + egui::vec2(0.0, -d), c + egui::vec2(d, 0.0), c + egui::vec2(0.0, d), c + egui::vec2(-d, 0.0)];
        ui.painter().add(egui::Shape::convex_polygon(pts, Color32::from_rgb(40, 95, 190), egui::Stroke::NONE));
    }
}

impl SketchCadApp {
    /// Entity Info for a single group or component.
    pub fn instance_info(&mut self, ui: &mut egui::Ui, id: InstanceId) {
        let Some(inst) = self.doc.active_mesh().instances.get(id).cloned() else { return };
        let Some(def) = self.doc.model.definitions.get(inst.def).cloned() else { return };
        let count = self.doc.model.instance_count(inst.def);
        let mut new_inst = inst.clone();
        let mut new_def_name = def.name.clone();
        let mut new_desc = def.description.clone();
        let tags = self.doc.model.tags.clone();
        egui::Grid::new("instance_info").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            let kind = if def.is_group { "Group".to_string() } else { format!("Component ({count} in model)") };
            ui.label(RichText::new(kind).strong());
            ui.end_row();
            ui.label("Tag");
            egui::ComboBox::from_id_salt("inst_tag").selected_text(tag_label(&tags, new_inst.tag)).show_ui(ui, |ui| {
                ui.selectable_value(&mut new_inst.tag, None, "Untagged");
                for (i, t) in tags.iter().enumerate() {
                    ui.selectable_value(&mut new_inst.tag, Some(i as u32), &t.name);
                }
            });
            ui.end_row();
            ui.label("Instance");
            ui.text_edit_singleline(&mut new_inst.name);
            ui.end_row();
            if def.is_group {
                ui.label("Name");
                ui.text_edit_singleline(&mut new_def_name);
                ui.end_row();
            } else {
                ui.label("Definition");
                ui.text_edit_singleline(&mut new_def_name);
                ui.end_row();
                ui.label("Description");
                ui.text_edit_singleline(&mut new_desc);
                ui.end_row();
            }
            let b = self.doc.model.instance_bounds(&inst);
            if !b.is_empty() {
                let u = &self.doc.model.units;
                let d = b.max - b.min;
                ui.label("Size");
                ui.label(format!("{} × {} × {}", u.format(d.x), u.format(d.y), u.format(d.z)));
                ui.end_row();
            }
            ui.checkbox(&mut new_inst.hidden, "Hidden");
            ui.checkbox(&mut new_inst.locked, "Locked");
            ui.end_row();
        });
        let inst_changed = new_inst.tag != inst.tag || new_inst.name != inst.name || new_inst.hidden != inst.hidden || new_inst.locked != inst.locked;
        if inst_changed {
            self.doc.checkpoint("Entity Info");
            let m = self.doc.active_mesh_mut();
            m.instances[id] = new_inst;
            m.revision += 1;
        }
        if new_def_name != def.name || new_desc != def.description {
            let d = &mut self.doc.model.definitions[inst.def];
            d.name = new_def_name;
            d.description = new_desc;
            self.doc.dirty = true;
        }
    }

    /// In-model component definitions; click one to place a copy with the Move tool.
    pub fn components_panel(&mut self, ui: &mut egui::Ui) {
        let used = self.doc.model.used_definitions();
        let mut defs: Vec<(DefId, String, usize)> = self
            .doc
            .model
            .definitions
            .iter()
            .filter(|(id, d)| !d.is_group && used.contains(id))
            .map(|(id, d)| (id, d.name.clone(), self.doc.model.instance_count(id)))
            .collect();
        defs.sort_by(|a, b| a.1.cmp(&b.1));
        if defs.is_empty() {
            ui.label(RichText::new("No components in model.\nSelect geometry and press G to make one.").weak());
            return;
        }
        for (id, name, count) in defs {
            if ui.selectable_label(false, format!("{name}   ({count})")).on_hover_text("Click to place a copy").clicked() {
                self.place_component(id);
            }
        }
    }

    /// Insert a new instance of `def` at the origin and switch to Move so it can be positioned.
    fn place_component(&mut self, def: DefId) {
        self.with_tool_cx(|t, cx| t.cancel(cx));
        self.doc.checkpoint("Place Component");
        let m = self.doc.active_mesh_mut();
        let id = m.instances.insert(sc_geom::Instance::new(def, sc_math::DMat4::IDENTITY));
        m.revision += 1;
        self.doc.selection.clear();
        self.doc.selection.instances.insert(id);
        self.set_tool(crate::tools::ToolId::Move);
    }

    pub fn tags_panel(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut changed = false;
        egui::Grid::new("tags").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("Untagged").italics());
            ui.end_row();
            for (i, t) in self.doc.model.tags.iter_mut().enumerate() {
                changed |= ui.checkbox(&mut t.visible, "").on_hover_text("Visible").changed();
                changed |= ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(120.0)).changed();
                let c = Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], t.color[3]);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c);
                    if ui.small_button("x").on_hover_text("Delete tag").clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if ui.button("Add Tag").clicked() {
            let n = self.doc.model.tags.len();
            let name = (1..).map(|k| format!("Tag{k}")).find(|nm| !self.doc.model.tags.iter().any(|t| &t.name == nm)).unwrap();
            self.doc.model.tags.push(Tag { name, visible: true, color: TAG_COLORS[n % TAG_COLORS.len()] });
            changed = true;
        }
        if let Some(i) = remove {
            self.delete_tag(i as u32);
            changed = true;
        }
        if changed {
            self.doc.dirty = true;
        }
    }

    /// Remove a tag; its entities become Untagged and higher tag indices shift down.
    fn delete_tag(&mut self, idx: u32) {
        self.doc.model.tags.remove(idx as usize);
        let fix = |m: &mut sc_geom::Mesh| {
            for i in m.instances.values_mut() {
                i.tag = match i.tag {
                    Some(t) if t == idx => None,
                    Some(t) if t > idx => Some(t - 1),
                    other => other,
                };
            }
            m.revision += 1;
        };
        fix(&mut self.doc.model.mesh);
        for d in self.doc.model.definitions.values_mut() {
            fix(&mut d.mesh);
        }
    }

    /// Tree of groups and components. Click to select (in the active context), double-click to open.
    pub fn outliner(&mut self, ui: &mut egui::Ui) {
        let root = &self.doc.model.mesh;
        if root.instances.is_empty() {
            ui.label(RichText::new("No groups or components.").weak());
            return;
        }
        let mut clicked: Option<(Vec<InstanceId>, bool)> = None;
        self.outliner_level(ui, None, &mut Vec::new(), &mut clicked);
        if let Some((path, double)) = clicked {
            self.with_tool_cx(|t, cx| t.cancel(cx));
            let (last, parents) = path.split_last().unwrap();
            self.doc.edit_path = parents.to_vec();
            self.doc.selection.clear();
            if double {
                self.doc.enter(*last);
            } else {
                self.doc.selection.instances.insert(*last);
            }
        }
    }

    fn outliner_level(&self, ui: &mut egui::Ui, ctx: Option<DefId>, path: &mut Vec<InstanceId>, clicked: &mut Option<(Vec<InstanceId>, bool)>) {
        let m = &self.doc.model;
        let mesh = m.context_mesh(ctx);
        let mut items: Vec<_> = mesh.instances.iter().collect();
        items.sort_by_key(|(_, i)| m.definitions.get(i.def).map(|d| d.name.clone()).unwrap_or_default());
        for (id, inst) in items {
            let Some(def) = m.definitions.get(inst.def) else { continue };
            path.push(id);
            let label = if inst.name.is_empty() { def.name.clone() } else { format!("{} <{}>", inst.name, def.name) };
            let selected = self.doc.edit_path.len() + 1 == path.len() && self.doc.edit_path[..] == path[..path.len() - 1] && self.doc.selection.instances.contains(&id);
            let open = self.doc.edit_path.starts_with(path);
            let text = RichText::new(label);
            let text = if inst.hidden || !m.tag_visible(inst.tag) { text.weak().italics() } else { text };
            let is_group = def.is_group;
            let mut row = |ui: &mut egui::Ui| {
                outliner_icon(ui, is_group);
                let r = ui.selectable_label(selected || open, text.clone());
                if r.double_clicked() {
                    *clicked = Some((path.clone(), true));
                } else if r.clicked() {
                    *clicked = Some((path.clone(), false));
                }
            };
            if def.mesh.instances.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    row(ui);
                });
            } else {
                let id_salt = ui.make_persistent_id(path.clone());
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id_salt, open)
                    .show_header(ui, |ui| row(ui))
                    .body(|ui| self.outliner_level(ui, Some(inst.def), path, clicked));
            }
            path.pop();
        }
    }
}
