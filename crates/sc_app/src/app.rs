//! Application state and per-frame orchestration.

use crate::commands::{Command, Keymap};
use crate::document::Document;
use crate::inference::InferenceEngine;
use crate::tools::{Tool, ToolId, Vcb, make_tool};
use sc_geom::Color;
use sc_math::DVec2;
use sc_render::{FaceStyle, StandardView, ViewportRenderer};
use std::path::PathBuf;
use std::time::Instant;

/// Command-line options.
#[derive(Default)]
pub struct LaunchOptions {
    pub open: Option<PathBuf>,
    /// Build the sample model.
    pub demo: bool,
    /// Save a PNG of the window after start-up and exit (visual checks / docs).
    pub screenshot: Option<PathBuf>,
    /// Tool to activate at start-up (by name, e.g. "Push/Pull").
    pub tool: Option<String>,
    /// Start Ruby and load installed SketchUp extensions.
    pub extensions: bool,
}

impl LaunchOptions {
    pub fn from_args(args: impl Iterator<Item = std::ffi::OsString>) -> Self {
        let mut o = Self { extensions: true, ..Self::default() };
        let mut args = args.peekable();
        while let Some(a) = args.next() {
            match a.to_str() {
                Some("--demo") => o.demo = true,
                Some("--screenshot") => o.screenshot = args.next().map(PathBuf::from),
                Some("--tool") => o.tool = args.next().and_then(|s| s.into_string().ok()),
                Some("--no-extensions") => o.extensions = false,
                _ => o.open = Some(PathBuf::from(a)),
            }
        }
        o
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    New,
    Open,
    Quit,
}

pub struct Panels {
    pub getting_started: bool,
    pub large_tool_set: bool,
    pub tray: bool,
    pub model_info: bool,
    pub preferences: bool,
    pub about: bool,
}

pub struct SketchCadApp {
    pub doc: Document,
    pub tool: Box<dyn Tool>,
    pub inference: InferenceEngine,
    pub vcb: Vcb,
    /// Characters typed into the Measurements box but not yet submitted.
    pub vcb_text: String,
    pub vcb_typing: bool,
    pub message: Option<(String, Instant, bool)>,
    pub material: Color,
    pub keymap: Keymap,
    pub renderer: Option<ViewportRenderer>,
    pub panels: Panels,
    pub pending: Option<Pending>,
    pub error_dialog: Option<String>,
    pub allow_close: bool,
    // Viewport interaction state.
    pub viewport: crate::ui::viewport::ViewportState,
    pub prev_mods: egui::Modifiers,
    /// The current inference lock came from an arrow key (persists after Shift is released).
    pub arrow_locked: bool,
    /// (revision key, flattened geometry outside the active context, in its local space).
    pub extra: (u64, sc_geom::Mesh),
    /// Make Component dialog: (name, description) being edited.
    pub component_dialog: Option<(String, String)>,
    pub tool_request: Option<crate::tools::ToolRequest>,
    pub active_scene: Option<usize>,
    pub camera_anim: Option<crate::scenes::CameraAnimation>,
    /// Frame the model once the viewport size is known.
    pub pending_zoom_extents: bool,
    pub plugins: crate::plugins::Plugins,
    /// A SketchUp file being opened in the background.
    pub skp_open: Option<(PathBuf, std::sync::mpsc::Receiver<crate::skp::OpenResult>)>,
    screenshot: Option<(PathBuf, u32)>,
}

impl SketchCadApp {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: LaunchOptions) -> Self {
        crate::ui::apply_theme(&cc.egui_ctx);
        let renderer = cc.wgpu_render_state.as_ref().map(ViewportRenderer::new);
        let mut app = Self {
            doc: Document::default(),
            tool: make_tool(ToolId::Select),
            inference: InferenceEngine::default(),
            vcb: Vcb::default(),
            vcb_text: String::new(),
            vcb_typing: false,
            message: None,
            material: [178, 34, 34, 255],
            keymap: Keymap::default(),
            renderer,
            panels: Panels { getting_started: true, large_tool_set: true, tray: true, model_info: false, preferences: false, about: false },
            pending: None,
            error_dialog: None,
            allow_close: false,
            viewport: Default::default(),
            prev_mods: Default::default(),
            arrow_locked: false,
            extra: (u64::MAX, sc_geom::Mesh::new()),
            component_dialog: None,
            tool_request: None,
            active_scene: None,
            camera_anim: None,
            pending_zoom_extents: false,
            plugins: crate::plugins::Plugins::new(crate::plugins::manager::Dirs::user()),
            skp_open: None,
            screenshot: opts.screenshot.map(|p| (p, 0)),
        };
        if opts.extensions {
            app.plugins.start();
        }
        if let Some(p) = opts.open {
            app.open_path(&p);
        }
        if opts.demo {
            use crate::document::Definition;
            let m = &mut app.doc.model;
            let house = m.definitions.insert(Definition { name: "House".into(), description: String::new(), is_group: true, mesh: crate::demo::house() });
            let bush = m.definitions.insert(Definition { name: "Bush".into(), description: "Octagonal shrub".into(), is_group: false, mesh: crate::demo::bush() });
            m.mesh.instances.insert(sc_geom::Instance::new(house, sc_math::DMat4::IDENTITY));
            for x in [-1400.0, 6600.0, 7700.0] {
                let at = sc_math::DVec3::new(x / 25.4, -1500.0 / 25.4, 0.0);
                m.mesh.instances.insert(sc_geom::Instance::new(bush, sc_math::DMat4::from_translation(at)));
            }
            app.doc.model.camera.eye = sc_math::DVec3::new(-150.0, -320.0, 220.0);
            app.doc.model.camera.target = sc_math::DVec3::new(118.0, 79.0, 70.0);
            app.pending_zoom_extents = true;
        }
        if let Some(name) = opts.tool {
            let all = [ToolId::Select, ToolId::Eraser, ToolId::PaintBucket, ToolId::Line, ToolId::Arc, ToolId::Rectangle, ToolId::Circle, ToolId::Polygon, ToolId::PushPull, ToolId::Move, ToolId::Rotate, ToolId::Scale, ToolId::FollowMe, ToolId::Offset, ToolId::TapeMeasure, ToolId::Orbit, ToolId::Pan, ToolId::Zoom];
            if let Some(t) = all.into_iter().find(|t| t.name().eq_ignore_ascii_case(&name)) {
                app.tool = make_tool(t);
            }
        }
        app.vcb.label = app.tool.vcb_label();
        app
    }

    pub fn set_tool(&mut self, id: ToolId) {
        if self.tool.id() == id {
            return;
        }
        self.with_tool_cx(|tool, cx| tool.deactivate(cx));
        self.tool = make_tool(id);
        self.inference.reset();
        self.arrow_locked = false;
        self.vcb.label = self.tool.vcb_label();
        self.vcb.value.clear();
        self.vcb_text.clear();
        self.vcb_typing = false;
        self.plugins.status_text = None;
    }

    pub fn flash(&mut self, msg: impl Into<String>, error: bool) {
        self.message = Some((msg.into(), Instant::now(), error));
    }

    /// Run `f` with the active tool and a tool context for the active editing context.
    ///
    /// The active context's mesh is moved out of the model for the duration of the call so the
    /// tool can mutate it while reading every other definition.
    pub fn with_tool_cx<R>(&mut self, f: impl FnOnce(&mut dyn Tool, &mut crate::tools::ToolCx) -> R) -> R {
        self.refresh_extra();
        let doc = &mut self.doc;
        let ctx = doc.active_ctx();
        let to_world = doc.active_xform();
        let mut mesh = std::mem::take(doc.model.context_mesh_mut(ctx));
        let camera = doc.model.camera.to_local(&to_world);
        let model = &mut doc.model;
        let mut cx = crate::tools::ToolCx {
            mesh: &mut mesh,
            view: crate::tools::ModelView { defs: &model.definitions, tags: &model.tags },
            extra: &self.extra.1,
            camera,
            world_camera: &mut model.camera,
            to_world,
            ctx,
            selection: &mut doc.selection,
            history: &mut doc.history,
            guides: &mut model.guides,
            dirty: &mut doc.dirty,
            units: model.units,
            size: self.viewport.size,
            cursor: self.viewport.cursor,
            inference: &mut self.inference,
            mods: self.prev_mods,
            vcb: &mut self.vcb,
            material: &mut self.material,
            delta: DVec2::ZERO,
            request: &mut self.tool_request,
        };
        let r = f(self.tool.as_mut(), &mut cx);
        *self.doc.model.context_mesh_mut(ctx) = mesh;
        match self.tool_request.take() {
            Some(crate::tools::ToolRequest::Enter(i)) => self.doc.enter(i),
            Some(crate::tools::ToolRequest::Exit) => {
                self.doc.exit();
            }
            None => {}
        }
        r
    }

    /// Rebuild the flattened "everything but the active context" mesh used for inference and
    /// picking. It stays frozen while a tool operation is in progress, so geometry being moved
    /// never snaps to its own (moving) copy.
    pub fn refresh_extra(&mut self) {
        let key = self.doc.revision_key();
        if key == self.extra.0 || self.tool.busy() {
            return;
        }
        self.extra = (key, crate::scene::flatten_outside_active(&self.doc));
    }

    pub fn run(&mut self, ctx: &egui::Context, cmd: Command) {
        match cmd {
            Command::Tool(t) => self.set_tool(t),
            Command::New => self.guard_dirty(Pending::New),
            Command::Open => self.guard_dirty(Pending::Open),
            Command::Quit => self.guard_dirty(Pending::Quit),
            Command::Save => {
                self.save(false);
            }
            Command::SaveAs => {
                self.save(true);
            }
            Command::Undo => {
                self.with_tool_cx(|t, cx| t.cancel(cx));
                let name = self.doc.history.undo_name().map(str::to_string);
                if self.doc.undo() {
                    self.flash(format!("Undo {}", name.unwrap_or_default()), false);
                }
            }
            Command::Redo => {
                self.with_tool_cx(|t, cx| t.cancel(cx));
                let name = self.doc.history.redo_name().map(str::to_string);
                if self.doc.redo() {
                    self.flash(format!("Redo {}", name.unwrap_or_default()), false);
                }
            }
            Command::Delete => self.delete_selection(),
            Command::SelectAll => {
                let m = self.doc.active_mesh();
                let sel = crate::document::Selection {
                    edges: m.edges.iter().filter(|(_, e)| !e.hidden).map(|(id, _)| id).collect(),
                    faces: m.faces.keys().collect(),
                    instances: m.instances.iter().filter(|(_, i)| !i.hidden).map(|(id, _)| id).collect(),
                };
                self.doc.selection = sel;
            }
            Command::SelectNone => self.doc.selection.clear(),
            Command::Hide => {
                if !self.doc.selection.is_empty() {
                    self.doc.checkpoint("Hide");
                    let sel = std::mem::take(&mut self.doc.selection);
                    let m = self.doc.active_mesh_mut();
                    for e in sel.edges {
                        m.edges[e].hidden = true;
                    }
                    for i in sel.instances {
                        m.instances[i].hidden = true;
                    }
                    m.revision += 1;
                }
            }
            Command::UnhideAll => {
                self.doc.checkpoint("Unhide All");
                let m = self.doc.active_mesh_mut();
                for e in m.edges.values_mut() {
                    e.hidden = false;
                }
                for i in m.instances.values_mut() {
                    i.hidden = false;
                }
                m.revision += 1;
            }
            Command::ReverseFaces => {
                if !self.doc.selection.faces.is_empty() {
                    self.doc.checkpoint("Reverse Faces");
                    for f in self.doc.selection.faces.clone() {
                        self.doc.active_mesh_mut().reverse_face(f);
                    }
                }
            }
            Command::MakeGroup => self.make_definition(true, None, String::new()),
            Command::MakeComponent => {
                if !self.doc.selection.is_empty() {
                    self.component_dialog = Some((self.doc.model.unique_def_name("Component"), String::new()));
                }
            }
            Command::Explode => self.explode_selection(),
            Command::EditComponent => {
                if let Some(&i) = self.doc.selection.instances.iter().next() {
                    self.with_tool_cx(|t, cx| t.cancel(cx));
                    self.doc.enter(i);
                }
            }
            Command::CloseComponent => {
                self.with_tool_cx(|t, cx| t.cancel(cx));
                self.doc.exit();
            }
            Command::MakeUnique => self.make_unique(),
            Command::AddScene => self.add_scene(),
            Command::NextScene => self.step_scene(1),
            Command::PreviousScene => self.step_scene(-1),
            Command::DeleteGuides => {
                if !self.doc.model.guides.is_empty() {
                    self.doc.checkpoint("Delete Guides");
                    self.doc.model.guides.clear();
                }
            }
            Command::ZoomExtents => {
                let b = self.doc.model.bounds();
                let aspect = self.viewport.size.x / self.viewport.size.y.max(1.0);
                self.doc.model.camera.zoom_extents(&b, aspect);
            }
            Command::StandardView(v) => {
                let b = self.doc.model.bounds();
                let aspect = self.viewport.size.x / self.viewport.size.y.max(1.0);
                self.doc.model.camera.set_standard_view(v, &b, aspect);
            }
            Command::FaceStyle(s) => self.doc.model.style.face_style = s,
            Command::ToggleBackEdges => {
                let st = &mut self.doc.model.style;
                st.back_edges = !st.back_edges;
                if st.back_edges && st.face_style == FaceStyle::XRay {
                    st.face_style = FaceStyle::Shaded;
                }
            }
            Command::ToggleXRay => {
                let st = &mut self.doc.model.style;
                st.face_style = if st.face_style == FaceStyle::XRay { FaceStyle::Shaded } else { FaceStyle::XRay };
            }
            Command::ToggleAxes => self.doc.model.style.show_axes = !self.doc.model.style.show_axes,
            Command::ToggleGuides => self.doc.model.style.show_guides = !self.doc.model.style.show_guides,
            Command::ModelInfo => self.panels.model_info = true,
            Command::Preferences | Command::Shortcuts => self.panels.preferences = true,
            Command::About => self.panels.about = true,
        }
        let _ = ctx;
    }

    fn delete_selection(&mut self) {
        if self.doc.selection.is_empty() {
            return;
        }
        self.doc.checkpoint("Erase");
        let sel = std::mem::take(&mut self.doc.selection);
        let faces: Vec<_> = sel.faces.into_iter().collect();
        let edges: Vec<_> = sel.edges.into_iter().collect();
        let m = self.doc.active_mesh_mut();
        m.erase_faces(&faces);
        m.erase_edges(&edges);
        for i in sel.instances {
            m.instances.remove(i);
        }
        m.revision += 1;
    }

    fn guard_dirty(&mut self, action: Pending) {
        if self.doc.dirty {
            self.pending = Some(action);
        } else {
            self.perform(action);
        }
    }

    pub fn perform(&mut self, action: Pending) {
        match action {
            Pending::New => {
                self.with_tool_cx(|t, cx| t.cancel(cx));
                self.doc = Document::default();
                self.inference.reset();
            }
            Pending::Open => {
                let file = rfd::FileDialog::new()
                    .add_filter("All Models", &[crate::document::FILE_EXTENSION, "skp", "skb"])
                    .add_filter("SketchCAD Model", &[crate::document::FILE_EXTENSION])
                    .add_filter("SketchUp Model", &["skp", "skb"])
                    .pick_file();
                if let Some(p) = file {
                    self.open_path(&p);
                }
            }
            Pending::Quit => self.allow_close = true,
        }
    }

    pub fn open_path(&mut self, p: &std::path::Path) {
        if crate::skp::is_skp(p) {
            // The converter can take a few seconds on big models; keep the window responsive.
            let (tx, rx) = std::sync::mpsc::channel();
            let path = p.to_path_buf();
            std::thread::spawn(move || {
                let _ = tx.send(crate::skp::open(&path));
            });
            self.skp_open = Some((p.to_path_buf(), rx));
            return;
        }
        match Document::load(p) {
            Ok(doc) => {
                self.with_tool_cx(|t, cx| t.cancel(cx));
                self.doc = doc;
                self.inference.reset();
            }
            Err(e) => self.error_dialog = Some(format!("Could not open {}:\n{e}", p.display())),
        }
    }

    /// Returns true if the model was saved.
    pub fn save(&mut self, save_as: bool) -> bool {
        let path = match (&self.doc.path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let stem = self.doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
                let skp = self.doc.path.as_deref().is_some_and(crate::skp::is_skp);
                let ext = if skp { "skp" } else { crate::document::FILE_EXTENSION };
                let mut d = rfd::FileDialog::new();
                d = if skp {
                    d.add_filter("SketchUp Model", &["skp"]).add_filter("SketchCAD Model", &[crate::document::FILE_EXTENSION])
                } else {
                    d.add_filter("SketchCAD Model", &[crate::document::FILE_EXTENSION]).add_filter("SketchUp Model", &["skp"])
                };
                d.set_file_name(format!("{stem}.{ext}")).save_file()
            }
        };
        let Some(mut path) = path else { return false };
        if path.extension().is_none() {
            path.set_extension(crate::document::FILE_EXTENSION);
        }
        if crate::skp::is_skp(&path) {
            return match crate::skp::save(&self.doc.model, &path) {
                Ok(backup) => {
                    self.doc.path = Some(path.clone());
                    self.doc.dirty = false;
                    let note = backup.map(|b| format!(" (previous version kept as {})", b.file_name().unwrap_or_default().to_string_lossy())).unwrap_or_default();
                    self.flash(format!("Saved {}{note}", path.display()), false);
                    true
                }
                Err(e) => {
                    self.error_dialog = Some(format!("Could not save {}:\n{e}", path.display()));
                    false
                }
            };
        }
        match self.doc.save(&path) {
            Ok(()) => {
                self.flash(format!("Saved {}", path.display()), false);
                true
            }
            Err(e) => {
                self.error_dialog = Some(format!("Could not save {}:\n{e}", path.display()));
                false
            }
        }
    }

    pub fn standard_views() -> [(StandardView, crate::ui::icons::Icon); 6] {
        use crate::ui::icons::Icon;
        [
            (StandardView::Iso, Icon::ViewIso),
            (StandardView::Top, Icon::ViewTop),
            (StandardView::Front, Icon::ViewFront),
            (StandardView::Right, Icon::ViewRight),
            (StandardView::Back, Icon::ViewBack),
            (StandardView::Left, Icon::ViewLeft),
        ]
    }
}

impl eframe::App for SketchCadApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Closing the window with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close
            && self.doc.dirty {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.pending = Some(Pending::Quit);
            }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.doc.title()));

        // Edits, undo and context changes can delete selected entities; drop stale ids before any
        // panel reads the selection.
        let mut sel = std::mem::take(&mut self.doc.selection);
        sel.prune(self.doc.active_mesh());
        self.doc.selection = sel;
        self.pump_plugins(&ctx);
        self.poll_skp_open(&ctx);

        self.menu_bar(ui);
        if self.panels.getting_started {
            self.top_toolbar(ui);
        }
        self.plugin_toolbars(ui);
        self.status_bar(ui);
        if self.panels.large_tool_set {
            self.large_tool_set(ui);
        }
        if self.panels.tray {
            self.tray(ui);
        }
        self.animate_camera(&ctx);
        self.viewport_ui(ui, frame);
        self.dialogs(&ctx);
        self.plugin_windows(&ctx);
        if self.pending_zoom_extents && self.viewport.size.x > 1.0 {
            self.pending_zoom_extents = false;
            self.run(&ctx, Command::ZoomExtents);
        }
        self.automation(&ctx);
    }
}

impl SketchCadApp {
    /// `--screenshot`: wait for the window to settle, grab it, save a PNG and quit.
    fn automation(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = &mut self.screenshot else { return };
        *frames += 1;
        if *frames == 20 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|b| b.save(&*path)) {
                Some(Ok(())) => log::info!("saved screenshot to {}", path.display()),
                other => eprintln!("screenshot failed: {other:?}"),
            }
            self.allow_close = true;
        }
        ctx.request_repaint();
    }
}
