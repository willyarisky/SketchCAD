//! Extension Manager, Ruby Console, dialogs extensions open, and the menus/toolbars they add.

use super::{Dialog, HostState, Job, LineKind, MenuEntry};
use crate::app::SketchCadApp;
use crate::commands::Command;
use crate::tools::ToolId;
use egui::{Color32, RichText};
use sc_render::StandardView;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What `Sketchup.send_action` names map to.
pub fn action_command(name: &str) -> Option<Command> {
    let t = Command::Tool;
    Some(match name.trim_end_matches(':') {
        "selectSelectionTool" => t(ToolId::Select),
        "selectEraseTool" => t(ToolId::Eraser),
        "selectPaintTool" => t(ToolId::PaintBucket),
        "selectLineTool" => t(ToolId::Line),
        "selectArcTool" => t(ToolId::Arc),
        "selectRectangleTool" => t(ToolId::Rectangle),
        "selectCircleTool" => t(ToolId::Circle),
        "selectPolygonTool" => t(ToolId::Polygon),
        "selectPushPullTool" => t(ToolId::PushPull),
        "selectMoveTool" => t(ToolId::Move),
        "selectRotateTool" => t(ToolId::Rotate),
        "selectScaleTool" => t(ToolId::Scale),
        "selectExtrudeTool" => t(ToolId::FollowMe),
        "selectOffsetTool" => t(ToolId::Offset),
        "selectMeasureTool" => t(ToolId::TapeMeasure),
        "selectOrbitTool" => t(ToolId::Orbit),
        "selectDollyTool" => t(ToolId::Pan),
        "selectZoomTool" => t(ToolId::Zoom),
        "viewZoomExtents" => Command::ZoomExtents,
        "viewIso" => Command::StandardView(StandardView::Iso),
        "viewTop" => Command::StandardView(StandardView::Top),
        "viewFront" => Command::StandardView(StandardView::Front),
        "viewBack" => Command::StandardView(StandardView::Back),
        "viewLeft" => Command::StandardView(StandardView::Left),
        "viewRight" => Command::StandardView(StandardView::Right),
        "editUndo" => Command::Undo,
        "editRedo" => Command::Redo,
        "editDelete" => Command::Delete,
        "editSelectAll" => Command::SelectAll,
        "editSelectNone" => Command::SelectNone,
        "editHide" => Command::Hide,
        "editUnhide" => Command::UnhideAll,
        "newDocument" => Command::New,
        _ => return None,
    })
}

impl SketchCadApp {
    /// Run Ruby's queued work for this frame and apply what it asked of the app.
    pub fn pump_plugins(&mut self, ctx: &egui::Context) {
        let view = super::api::ViewInfo { size: [self.viewport.size.x as f32, self.viewport.size.y as f32] };
        if self.plugins.pump(&mut self.doc, &view, std::time::Duration::from_millis(30)) {
            ctx.request_repaint();
        }
        if self.plugins.busy() {
            ctx.request_repaint();
        } else if let Some(at) = self.plugins.next_timer() {
            ctx.request_repaint_after(at.saturating_duration_since(std::time::Instant::now()));
        }
        for (msg, err) in std::mem::take(&mut self.plugins.notices) {
            self.flash(msg, err);
        }
        for a in std::mem::take(&mut self.plugins.actions) {
            match action_command(&a) {
                Some(cmd) => self.run(ctx, cmd),
                None => self.plugins.log(LineKind::Error, format!("Sketchup.send_action: '{a}' is not supported")),
            }
        }
    }

    /// Run an extension's menu item / toolbar button.
    pub fn run_plugin_command(&mut self, proc_id: u64, label: String) {
        self.with_tool_cx(|t, cx| t.cancel(cx));
        self.plugins.enqueue(Job::Call { proc_id, label });
    }

    pub fn extensions_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Extension Manager...").clicked() {
            self.plugins.show_manager = true;
        }
        if ui.button("Ruby Console").clicked() {
            self.plugins.show_console = true;
        }
        if self.plugins.menus.iter().any(|m| m.menu() == "Extensions") {
            ui.separator();
            self.plugin_menu_items(ui, "Extensions");
        }
    }

    /// Items extensions added to the menu `menu`, with their submenus.
    pub fn plugin_menu_items(&mut self, ui: &mut egui::Ui, menu: &str) {
        let entries: Vec<MenuEntry> = self.plugins.menus.iter().filter(|e| e.menu() == menu).cloned().collect();
        if entries.is_empty() {
            return;
        }
        if menu != "Extensions" {
            ui.separator();
        }
        let refs: Vec<&MenuEntry> = entries.iter().collect();
        if let Some((id, label)) = menu_level(ui, &refs, 0) {
            self.run_plugin_command(id, label);
        }
    }

    /// Toolbars extensions created and showed.
    pub fn plugin_toolbars(&mut self, ui: &mut egui::Ui) {
        let visible: Vec<usize> = (0..self.plugins.toolbars.len()).filter(|i| self.plugins.toolbars[*i].visible && !self.plugins.toolbars[*i].items.is_empty()).collect();
        if visible.is_empty() {
            return;
        }
        let mut clicked = None;
        egui::Panel::top("plugin_toolbars").frame(egui::Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(6, 2))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for i in visible {
                    let items = self.plugins.toolbars[i].items.clone();
                    for item in items {
                        let Some(proc_id) = item.proc_id else {
                            ui.separator();
                            continue;
                        };
                        let tip = if item.tooltip.is_empty() { item.label.clone() } else { item.tooltip.clone() };
                        let tex = item.icon.as_deref().and_then(|p| self.plugins.icon(ui.ctx(), p));
                        let resp = match tex {
                            Some(t) => ui.add(egui::Button::image(egui::Image::new(&t).fit_to_exact_size(egui::vec2(24.0, 24.0)))),
                            None => ui.button(&item.label),
                        };
                        if resp.on_hover_text(tip).clicked() {
                            clicked = Some((proc_id, item.label.clone()));
                        }
                    }
                    ui.separator();
                }
            });
        });
        if let Some((id, label)) = clicked {
            self.run_plugin_command(id, label);
        }
    }

    pub fn plugin_windows(&mut self, ctx: &egui::Context) {
        self.extension_manager(ctx);
        self.ruby_console(ctx);
        self.install_confirm(ctx);
        self.plugin_dialog(ctx);
    }

    fn extension_manager(&mut self, ctx: &egui::Context) {
        let mut open = self.plugins.show_manager;
        let mut toggle: Option<(String, bool)> = None;
        let mut uninstall: Option<String> = None;
        let mut restart = false;
        egui::Window::new("Extension Manager").open(&mut open).default_size([520.0, 420.0]).default_pos([140.0, 120.0]).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Install Extension...").clicked() {
                    let file = rfd::FileDialog::new().add_filter("SketchUp Extension", &["rbz", "zip", "rb"]).pick_file();
                    self.plugins.confirm_install = file;
                }
                if ui.button("Restart Ruby").on_hover_text("Reload every extension").clicked() {
                    restart = true;
                }
                if ui.button("Open Plugins Folder").clicked() {
                    open_folder(&self.plugins.dirs.plugins);
                }
            });
            match &self.plugins.state {
                HostState::Failed(e) => {
                    ui.colored_label(Color32::from_rgb(190, 30, 30), e);
                }
                HostState::Off => {
                    ui.label("Extensions are turned off for this session.");
                }
                HostState::Starting => {
                    ui.label("Loading extensions...");
                }
                HostState::Ready => {}
            }
            ui.separator();
            if self.plugins.extensions.is_empty() {
                ui.label("No extensions installed. Install a SketchUp extension (.rbz) to add tools and commands.");
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for ext in &self.plugins.extensions {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut on = ext.enabled;
                            if ui.checkbox(&mut on, "").on_hover_text(if on { "Disable" } else { "Enable" }).changed() {
                                toggle = Some((ext.name.clone(), on));
                            }
                            ui.label(RichText::new(&ext.name).strong());
                            if !ext.version.is_empty() {
                                ui.label(RichText::new(format!("v{}", ext.version)).weak());
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Uninstall").clicked() {
                                    uninstall = Some(ext.name.clone());
                                }
                                let (txt, col) = match (ext.enabled, ext.loaded) {
                                    (true, true) => ("Loaded", Color32::from_rgb(20, 120, 50)),
                                    (true, false) => ("Failed to load — see Ruby Console", Color32::from_rgb(190, 30, 30)),
                                    (false, _) => ("Disabled", Color32::GRAY),
                                };
                                ui.label(RichText::new(txt).color(col).small());
                            });
                        });
                        if !ext.description.is_empty() {
                            ui.label(&ext.description);
                        }
                        let by: Vec<&str> = [ext.creator.as_str(), ext.copyright.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
                        if !by.is_empty() {
                            ui.label(RichText::new(by.join(" · ")).weak().small());
                        }
                    });
                }
            });
        });
        self.plugins.show_manager = open;
        if restart {
            self.plugins.restart(&mut self.doc);
        }
        if let Some((name, on)) = toggle {
            self.plugins.set_enabled(&mut self.doc, &name, on);
        }
        if let Some(name) = uninstall {
            self.plugins.uninstall(&mut self.doc, &name);
        }
    }

    fn install_confirm(&mut self, ctx: &egui::Context) {
        let Some(path) = self.plugins.confirm_install.clone() else { return };
        let mut done = None;
        egui::Modal::new(egui::Id::new("install_extension")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading("Install Extension");
            ui.label(format!("Install {}?", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            ui.add_space(4.0);
            ui.label("Extensions are programs: they can read and change your files. Only install extensions from developers you trust.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Install").clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                self.plugins.confirm_install = None;
                self.with_tool_cx(|t, cx| t.cancel(cx));
                self.plugins.install(&path);
            }
            Some(false) => self.plugins.confirm_install = None,
            None => {}
        }
    }

    fn ruby_console(&mut self, ctx: &egui::Context) {
        let mut open = self.plugins.show_console;
        let mut submit = None;
        egui::Window::new("Ruby Console").open(&mut open).default_size([560.0, 320.0]).default_pos([140.0, 520.0]).show(ctx, |ui| {
            let input_h = 28.0;
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).max_height(ui.available_height() - input_h).show(ui, |ui| {
                for (kind, line) in &self.plugins.console {
                    let col = match kind {
                        LineKind::Input => Color32::from_rgb(40, 40, 160),
                        LineKind::Result => Color32::from_rgb(20, 110, 40),
                        LineKind::Error => Color32::from_rgb(190, 30, 30),
                        LineKind::Info => Color32::GRAY,
                        LineKind::Output => ui.visuals().text_color(),
                    };
                    ui.label(RichText::new(line).monospace().color(col));
                }
            });
            ui.separator();
            let resp = ui.add(egui::TextEdit::singleline(&mut self.plugins.console_input).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).hint_text("Ruby code, e.g. Sketchup.active_model.entities.length"));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = Some(std::mem::take(&mut self.plugins.console_input));
                resp.request_focus();
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                && let Some(last) = self.plugins.console_history.last() {
                    self.plugins.console_input = last.clone();
                }
        });
        self.plugins.show_console = open;
        if let Some(code) = submit.filter(|c| !c.trim().is_empty()) {
            self.plugins.console_history.push(code.clone());
            self.with_tool_cx(|t, cx| t.cancel(cx));
            self.plugins.enqueue(Job::Eval(code));
        }
    }

    /// `UI.messagebox` / `UI.inputbox` while the extension waits for the answer.
    fn plugin_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.plugins.dialog.clone() else { return };
        let mut answer: Option<Value> = None;
        let mut edited = dialog.clone();
        egui::Modal::new(egui::Id::new("plugin_dialog")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            match &mut edited {
                Dialog::Message { text, kind, .. } => {
                    ui.label(text.as_str());
                    ui.add_space(8.0);
                    // MB_OK 0, OKCANCEL 1, ABORTRETRYIGNORE 2, YESNOCANCEL 3, YESNO 4, RETRYCANCEL 5.
                    let buttons: &[(&str, i64)] = match *kind & 0x0f {
                        1 => &[("OK", 1), ("Cancel", 2)],
                        2 => &[("Abort", 3), ("Retry", 4), ("Ignore", 5)],
                        3 => &[("Yes", 6), ("No", 7), ("Cancel", 2)],
                        4 => &[("Yes", 6), ("No", 7)],
                        5 => &[("Retry", 4), ("Cancel", 2)],
                        _ => &[("OK", 1)],
                    };
                    ui.horizontal(|ui| {
                        for (label, id) in buttons {
                            if ui.button(*label).clicked() {
                                answer = Some(Value::from(*id));
                            }
                        }
                    });
                }
                Dialog::Input { title, prompts, values, lists, .. } => {
                    ui.heading(title.as_str());
                    egui::Grid::new("plugin_inputbox").num_columns(2).show(ui, |ui| {
                        for (i, p) in prompts.iter().enumerate() {
                            ui.label(p);
                            if lists[i].is_empty() {
                                ui.text_edit_singleline(&mut values[i]);
                            } else {
                                egui::ComboBox::from_id_salt(("plugin_list", i)).selected_text(values[i].clone()).show_ui(ui, |ui| {
                                    for opt in &lists[i] {
                                        ui.selectable_value(&mut values[i], opt.clone(), opt);
                                    }
                                });
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            answer = Some(Value::from(values.clone()));
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            answer = Some(Value::Bool(false));
                        }
                    });
                }
            }
        });
        match answer {
            Some(v) => self.plugins.answer_dialog(v),
            None => self.plugins.dialog = Some(edited),
        }
    }
}

/// One level of an extension menu; returns the clicked item.
fn menu_level(ui: &mut egui::Ui, entries: &[&MenuEntry], depth: usize) -> Option<(u64, String)> {
    let mut clicked = None;
    let mut done_subs: Vec<String> = Vec::new();
    for e in entries {
        let path = match e {
            MenuEntry::Item { path, .. } | MenuEntry::Separator { path, .. } => path,
        };
        if path.len() > depth {
            let name = path[depth].clone();
            if done_subs.contains(&name) {
                continue;
            }
            done_subs.push(name.clone());
            let sub: Vec<&MenuEntry> = entries
                .iter()
                .copied()
                .filter(|x| match x {
                    MenuEntry::Item { path, .. } | MenuEntry::Separator { path, .. } => path.len() > depth && path[depth] == name,
                })
                .collect();
            ui.menu_button(&name, |ui| {
                if let Some(c) = menu_level(ui, &sub, depth + 1) {
                    clicked = Some(c);
                }
            });
            continue;
        }
        match e {
            MenuEntry::Item { label, proc_id, .. } => {
                if ui.button(label).clicked() {
                    clicked = Some((*proc_id, label.clone()));
                    ui.close();
                }
            }
            MenuEntry::Separator { .. } => {
                ui.separator();
            }
        }
    }
    clicked
}

fn open_folder(p: &Path) {
    let _ = std::fs::create_dir_all(p);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(p).spawn();
}

impl super::Plugins {
    /// Toolbar icon texture; SVG/PDF icons fall back to a PNG next to them.
    pub fn icon(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        if let Some(t) = self.icons.get(path) {
            return t.clone();
        }
        let png: PathBuf = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) { path.to_path_buf() } else { path.with_extension("png") };
        let tex = image::open(&png).ok().map(|img| {
            let img = img.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            ctx.load_texture(png.display().to_string(), egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
        });
        self.icons.insert(path.to_path_buf(), tex.clone());
        tex
    }
}
