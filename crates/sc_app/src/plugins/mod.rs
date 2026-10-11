//! SketchUp extensions: a Ruby interpreter running SketchCAD's implementation of SketchUp's Ruby
//! API, the Extension Manager, the Ruby Console, and the menus/toolbars extensions add.
//!
//! Ruby runs in a child process ([`host`]). Work for it is queued as jobs (console input, a menu
//! click, loading a file) and run one at a time; while a job runs, its model requests are answered
//! each frame ([`Plugins::pump`]) against the open document ([`api`]). Model changes made by a job
//! become undo steps: one per `start_operation`/`commit_operation`, plus one for any changes made
//! outside an operation.

pub mod api;
pub mod host;
pub mod manager;
pub mod ui;

use crate::document::{Document, UndoEntry};
use host::{HostMsg, RubyHost};
use manager::{Dirs, Settings};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// SketchCAD's Ruby API implementation, written to disk at start-up.
const RUBY_FILES: [(&str, &str); 5] = [
    ("bootstrap.rb", include_str!("ruby/bootstrap.rb")),
    ("sketchcad_api.rb", include_str!("ruby/sketchcad_api.rb")),
    ("sketchup.rb", include_str!("ruby/sketchup.rb")),
    ("extensions.rb", include_str!("ruby/extensions.rb")),
    ("langhandler.rb", include_str!("ruby/langhandler.rb")),
];

#[derive(Clone, Debug, PartialEq)]
pub enum HostState {
    /// Extensions are turned off for this session (tests, `--no-extensions`).
    Off,
    Starting,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Input,
    Output,
    Result,
    Error,
    Info,
}

#[derive(Clone, Debug)]
pub struct Extension {
    pub name: String,
    pub version: String,
    pub creator: String,
    pub description: String,
    pub copyright: String,
    /// The `path` given to `SketchupExtension.new` (what gets required).
    pub path: String,
    /// The `.rb` file that registered it.
    pub loader: Option<PathBuf>,
    pub loaded: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub enum MenuEntry {
    Item { menu: String, path: Vec<String>, label: String, proc_id: u64 },
    Separator { menu: String, path: Vec<String> },
}

impl MenuEntry {
    pub fn menu(&self) -> &str {
        match self {
            MenuEntry::Item { menu, .. } | MenuEntry::Separator { menu, .. } => menu,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolbarItem {
    pub label: String,
    pub tooltip: String,
    pub icon: Option<PathBuf>,
    /// `None` is a separator.
    pub proc_id: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct PluginToolbar {
    pub name: String,
    pub items: Vec<ToolbarItem>,
    pub visible: bool,
}

/// A dialog an extension is waiting on.
#[derive(Clone, Debug)]
pub enum Dialog {
    Message { req: u64, text: String, kind: u8 },
    Input { req: u64, title: String, prompts: Vec<String>, values: Vec<String>, lists: Vec<Vec<String>> },
}

#[derive(Clone, Debug)]
pub enum Job {
    Eval(String),
    Call { proc_id: u64, label: String },
    Load(PathBuf),
}

struct Timer {
    id: u64,
    proc_id: u64,
    at: Instant,
    every: Option<Duration>,
}

/// The job in progress and its undo bookkeeping.
struct Active {
    id: u64,
    label: String,
    /// Model before any change not yet recorded as an undo step.
    snap: UndoEntry,
    key: u64,
    /// Open `start_operation`: (name, model before it, revision key before it).
    op: Option<(String, UndoEntry, u64)>,
}

impl Active {
    fn new(id: u64, label: String, doc: &Document) -> Self {
        Self { id, label, snap: doc.model_snapshot(), key: doc.revision_key(), op: None }
    }

    /// Record changes made outside an operation as one undo step.
    fn flush(&mut self, doc: &mut Document) {
        if doc.revision_key() != self.key {
            let snap = std::mem::replace(&mut self.snap, doc.model_snapshot());
            doc.push_model_snapshot(&self.label, snap);
            self.key = doc.revision_key();
        }
    }

    fn start_operation(&mut self, doc: &mut Document, name: String) {
        self.commit(doc);
        self.flush(doc);
        self.op = Some((name, doc.model_snapshot(), doc.revision_key()));
    }

    fn commit(&mut self, doc: &mut Document) {
        if let Some((name, snap, key)) = self.op.take() {
            if doc.revision_key() != key {
                doc.push_model_snapshot(&name, snap);
            }
            self.snap = doc.model_snapshot();
            self.key = doc.revision_key();
        }
    }

    fn abort(&mut self, doc: &mut Document) {
        if let Some((_, snap, _)) = self.op.take() {
            doc.restore_snapshot(snap);
            self.snap = doc.model_snapshot();
            self.key = doc.revision_key();
        }
    }

    fn finish(mut self, doc: &mut Document) {
        self.commit(doc);
        self.flush(doc);
    }
}

pub struct Plugins {
    pub dirs: Dirs,
    pub settings: Settings,
    pub state: HostState,
    pub ruby: Option<PathBuf>,
    host: Option<RubyHost>,
    pub console: Vec<(LineKind, String)>,
    pub console_input: String,
    pub console_history: Vec<String>,
    pub extensions: Vec<Extension>,
    pub menus: Vec<MenuEntry>,
    pub toolbars: Vec<PluginToolbar>,
    pub dialog: Option<Dialog>,
    /// `Sketchup.status_text=`.
    pub status_text: Option<String>,
    /// `Sketchup.send_action` names for the app to perform.
    pub actions: Vec<String>,
    pub show_manager: bool,
    pub show_console: bool,
    /// A package picked for installation, awaiting the "only install trusted extensions" confirmation.
    pub confirm_install: Option<PathBuf>,
    /// Messages for the app to flash: (text, is_error).
    pub notices: Vec<(String, bool)>,
    icons: std::collections::HashMap<PathBuf, Option<egui::TextureHandle>>,
    jobs: VecDeque<Job>,
    active: Option<Active>,
    next_job: u64,
    timers: Vec<Timer>,
}

impl Plugins {
    pub fn new(dirs: Dirs) -> Self {
        let settings = Settings::load(&dirs);
        Self {
            dirs,
            settings,
            state: HostState::Off,
            ruby: None,
            host: None,
            console: Vec::new(),
            console_input: String::new(),
            console_history: Vec::new(),
            extensions: Vec::new(),
            menus: Vec::new(),
            toolbars: Vec::new(),
            dialog: None,
            status_text: None,
            actions: Vec::new(),
            show_manager: false,
            show_console: false,
            confirm_install: None,
            notices: Vec::new(),
            icons: Default::default(),
            jobs: VecDeque::new(),
            active: None,
            next_job: 1,
            timers: Vec::new(),
        }
    }

    pub fn log(&mut self, kind: LineKind, text: impl Into<String>) {
        for line in text.into().lines() {
            self.console.push((kind, line.to_string()));
        }
        let excess = self.console.len().saturating_sub(2000);
        self.console.drain(..excess);
    }

    /// Start the Ruby interpreter and load every installed extension.
    pub fn start(&mut self) {
        self.stop();
        let Some(ruby) = host::find_ruby() else {
            let msg = "Ruby was not found. Install Ruby (for example `brew install ruby`) or set SKETCHCAD_RUBY.";
            self.log(LineKind::Error, msg);
            self.state = HostState::Failed(msg.into());
            return;
        };
        let res = self.dirs.ensure().and_then(|_| {
            for (name, body) in RUBY_FILES {
                std::fs::write(self.dirs.ruby_api.join(name), body)?;
            }
            self.settings.save(&self.dirs)
        });
        if let Err(e) = res {
            self.state = HostState::Failed(format!("Could not prepare {}: {e}", self.dirs.data.display()));
            return;
        }
        let bootstrap = self.dirs.ruby_api.join("bootstrap.rb");
        match RubyHost::spawn(&ruby, &bootstrap, &[&self.dirs.plugins, &self.dirs.settings, &self.dirs.data]) {
            Ok(h) => {
                self.host = Some(h);
                self.state = HostState::Starting;
                self.log(LineKind::Info, format!("Ruby: {} — loading extensions from {}", ruby.display(), self.dirs.plugins.display()));
            }
            Err(e) => self.state = HostState::Failed(format!("Could not start {}: {e}", ruby.display())),
        }
        self.ruby = Some(ruby);
    }

    /// Stop Ruby and forget everything extensions registered.
    pub fn stop(&mut self) {
        if let Some(mut h) = self.host.take() {
            let _ = h.send(&json!({ "t": "quit" }));
            h.kill();
        }
        self.extensions.clear();
        self.menus.clear();
        self.toolbars.clear();
        self.timers.clear();
        self.jobs.clear();
        self.dialog = None;
        self.active = None;
        self.state = HostState::Off;
    }

    pub fn restart(&mut self, doc: &mut Document) {
        if let Some(a) = self.active.take() {
            a.finish(doc);
        }
        self.start();
    }

    pub fn enqueue(&mut self, job: Job) {
        if !matches!(self.state, HostState::Starting | HostState::Ready) {
            self.log(LineKind::Error, "Ruby is not running.");
            return;
        }
        self.jobs.push_back(job);
    }

    /// Ruby is starting, running a job, or has jobs queued.
    pub fn busy(&self) -> bool {
        self.state == HostState::Starting || self.active.is_some() || !self.jobs.is_empty()
    }

    pub fn next_timer(&self) -> Option<Instant> {
        self.timers.iter().map(|t| t.at).min()
    }

    fn reply(&mut self, id: u64, result: Result<Value, String>) {
        let msg = match result {
            Ok(v) => json!({ "t": "reply", "id": id, "ok": v }),
            Err(e) => json!({ "t": "reply", "id": id, "err": e }),
        };
        if let Some(h) = &mut self.host
            && let Err(e) = h.send(&msg) {
                self.log(LineKind::Error, format!("Lost contact with Ruby: {e}"));
            }
    }

    /// Answer the dialog an extension is waiting on.
    pub fn answer_dialog(&mut self, value: Value) {
        let req = match self.dialog.take() {
            Some(Dialog::Message { req, .. } | Dialog::Input { req, .. }) => req,
            None => return,
        };
        self.reply(req, Ok(value));
    }

    /// Process Ruby's messages for up to `budget`. Returns true if anything happened.
    pub fn pump(&mut self, doc: &mut Document, view: &api::ViewInfo, budget: Duration) -> bool {
        if self.host.is_none() {
            return false;
        }
        let deadline = Instant::now() + budget;
        let mut did = false;
        if self.state == HostState::Starting && self.active.is_none() {
            self.active = Some(Active::new(0, "Load Extensions".into(), doc));
        }
        self.fire_timers();
        loop {
            did |= self.start_next_job(doc);
            let waiting = self.active.is_some() && self.dialog.is_none();
            let now = Instant::now();
            let wait = if waiting && now < deadline { (deadline - now).min(Duration::from_millis(4)) } else { Duration::ZERO };
            let Some(h) = &self.host else { break };
            match h.recv(wait) {
                Some(msg) => {
                    did = true;
                    self.handle(msg, doc, view);
                }
                None if waiting && Instant::now() < deadline => continue,
                None => break,
            }
        }
        did
    }

    fn fire_timers(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        self.timers.retain_mut(|t| {
            if t.at > now {
                return true;
            }
            due.push(t.proc_id);
            match t.every {
                Some(d) => {
                    t.at = now + d;
                    true
                }
                None => false,
            }
        });
        for proc_id in due {
            self.jobs.push_back(Job::Call { proc_id, label: "Timer".into() });
        }
    }

    fn start_next_job(&mut self, doc: &Document) -> bool {
        if self.active.is_some() || self.state != HostState::Ready {
            return false;
        }
        let Some(job) = self.jobs.pop_front() else { return false };
        let id = self.next_job;
        self.next_job += 1;
        let (msg, label) = match job {
            Job::Eval(code) => {
                self.log(LineKind::Input, format!("> {code}"));
                (json!({ "t": "eval", "id": id, "code": code }), "Ruby Console".to_string())
            }
            Job::Call { proc_id, label } => (json!({ "t": "call", "id": id, "proc": proc_id }), label),
            Job::Load(path) => (json!({ "t": "load", "id": id, "path": path }), "Install Extension".to_string()),
        };
        self.active = Some(Active::new(id, label, doc));
        if let Some(h) = &mut self.host
            && let Err(e) = h.send(&msg) {
                self.log(LineKind::Error, format!("Lost contact with Ruby: {e}"));
                self.active = None;
                return false;
            }
        true
    }

    fn handle(&mut self, msg: HostMsg, doc: &mut Document, view: &api::ViewInfo) {
        match msg {
            HostMsg::Output(line) => self.log(LineKind::Output, line),
            HostMsg::Exited(why) => {
                if let Some(a) = self.active.take() {
                    a.finish(doc);
                }
                self.host = None;
                self.dialog = None;
                self.jobs.clear();
                self.state = HostState::Failed(why.clone());
                self.log(LineKind::Error, format!("{why}. Use Extensions > Restart Ruby to start it again."));
            }
            HostMsg::Msg(v) => match v["t"].as_str().unwrap_or_default() {
                "req" => {
                    let id = v["id"].as_u64().unwrap_or(0);
                    let m = v["m"].as_str().unwrap_or_default().to_string();
                    let empty = Vec::new();
                    let a = v["a"].as_array().unwrap_or(&empty);
                    if let Some(r) = self.request(doc, view, id, &m, a) {
                        self.reply(id, r);
                    }
                }
                "note" => {
                    let m = v["m"].as_str().unwrap_or_default().to_string();
                    self.note(doc, &m, &v["a"]);
                }
                "done" => {
                    let id = v["id"].as_u64().unwrap_or(0);
                    if let Some(a) = self.active.take_if(|a| a.id == id) {
                        a.finish(doc);
                    }
                    if v["ok"].as_bool() == Some(true) {
                        if let Some(r) = v["result"].as_str() {
                            self.log(LineKind::Result, r);
                        }
                    } else {
                        let err = v["err"].as_str().unwrap_or("error").to_string();
                        self.log(LineKind::Error, err.clone());
                        for line in v["bt"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                            self.log(LineKind::Error, format!("    {line}"));
                        }
                        self.notices.push((format!("Extension error: {err}"), true));
                    }
                }
                "ready" => {
                    if let Some(a) = self.active.take_if(|a| a.id == 0) {
                        a.finish(doc);
                    }
                    self.state = HostState::Ready;
                    let n = self.extensions.iter().filter(|e| e.loaded).count();
                    self.log(LineKind::Info, format!("Ready. {n} extension(s) loaded."));
                }
                other => self.log(LineKind::Error, format!("Unknown message from Ruby: {other}")),
            },
        }
    }

    /// Handle a request; `None` means the reply comes later (a dialog).
    fn request(&mut self, doc: &mut Document, view: &api::ViewInfo, id: u64, m: &str, a: &[Value]) -> Option<Result<Value, String>> {
        let s = |i: usize| a.get(i).and_then(Value::as_str).unwrap_or_default().to_string();
        Some(match m {
            "op.start" => {
                if let Some(act) = &mut self.active {
                    let name = Some(s(0)).filter(|n| !n.is_empty()).unwrap_or_else(|| act.label.clone());
                    act.start_operation(doc, name);
                }
                Ok(Value::Bool(true))
            }
            "op.commit" => {
                if let Some(act) = &mut self.active {
                    act.commit(doc);
                }
                Ok(Value::Bool(true))
            }
            "op.abort" => {
                if let Some(act) = &mut self.active {
                    act.abort(doc);
                }
                Ok(Value::Bool(true))
            }
            "ui.messagebox" => {
                let kind = a.get(1).and_then(Value::as_u64).unwrap_or(0) as u8;
                self.dialog = Some(Dialog::Message { req: id, text: s(0), kind });
                return None;
            }
            "ui.inputbox" => {
                let strs = |v: Option<&Value>| -> Vec<String> {
                    v.and_then(Value::as_array).into_iter().flatten().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect()
                };
                let prompts = strs(a.first());
                let mut values = strs(a.get(1));
                values.resize(prompts.len(), String::new());
                let mut lists: Vec<Vec<String>> = strs(a.get(2)).iter().map(|l| if l.is_empty() { Vec::new() } else { l.split('|').map(str::to_string).collect() }).collect();
                lists.resize(prompts.len(), Vec::new());
                for (v, l) in values.iter_mut().zip(&lists) {
                    if !l.is_empty() && !l.contains(v) {
                        *v = l[0].clone();
                    }
                }
                self.dialog = Some(Dialog::Input { req: id, title: Some(s(3)).filter(|t| !t.is_empty()).unwrap_or_else(|| "SketchCAD".into()), prompts, values, lists });
                return None;
            }
            "ui.openpanel" | "ui.savepanel" | "ui.select_directory" => {
                let mut d = rfd::FileDialog::new().set_title(s(0));
                if !s(1).is_empty() {
                    d = d.set_directory(s(1));
                }
                if !s(2).is_empty() && m != "ui.select_directory" {
                    d = d.set_file_name(s(2));
                }
                let picked = match m {
                    "ui.openpanel" => d.pick_file(),
                    "ui.savepanel" => d.save_file(),
                    _ => d.pick_folder(),
                };
                Ok(picked.map_or(Value::Null, |p| Value::String(p.display().to_string())))
            }
            "app.dirs" => Ok(json!({ "plugins": self.dirs.plugins, "data": self.dirs.data })),
            _ => api::call(doc, view, m, a),
        })
    }

    /// Notifications: things extensions register, which need no reply.
    fn note(&mut self, _doc: &mut Document, m: &str, a: &Value) {
        let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
        let path = || -> Vec<String> { a["path"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
        match m {
            "menu.item" => self.menus.push(MenuEntry::Item { menu: s("menu"), path: path(), label: s("label"), proc_id: a["proc"].as_u64().unwrap_or(0) }),
            "menu.separator" => self.menus.push(MenuEntry::Separator { menu: s("menu"), path: path() }),
            "toolbar.item" | "toolbar.separator" => {
                let name = s("toolbar");
                let icon = Some(s("icon")).filter(|i| !i.is_empty()).map(PathBuf::from);
                let item = ToolbarItem { label: s("label"), tooltip: s("tooltip"), icon, proc_id: a["proc"].as_u64() };
                let item = if m == "toolbar.separator" { ToolbarItem { proc_id: None, ..item } } else { item };
                match self.toolbars.iter_mut().find(|t| t.name == name) {
                    Some(t) => t.items.push(item),
                    None => self.toolbars.push(PluginToolbar { name, items: vec![item], visible: false }),
                }
            }
            "toolbar.visible" => {
                let name = s("toolbar");
                let vis = a["visible"].as_bool().unwrap_or(true);
                match self.toolbars.iter_mut().find(|t| t.name == name) {
                    Some(t) => t.visible = vis,
                    None => self.toolbars.push(PluginToolbar { name, items: Vec::new(), visible: vis }),
                }
            }
            "status" => self.status_text = Some(s("text")).filter(|t| !t.is_empty()),
            "action" => self.actions.push(s("name")),
            "ext.register" => {
                let name = s("name");
                let enabled = a["enabled"].as_bool().unwrap_or_else(|| !self.settings.disabled.contains(&name));
                let ext = Extension {
                    version: s("version"),
                    creator: s("creator"),
                    description: s("description"),
                    copyright: s("copyright"),
                    path: s("path"),
                    loader: Some(s("loader")).filter(|l| !l.is_empty()).map(PathBuf::from),
                    loaded: false,
                    enabled,
                    name: name.clone(),
                };
                match self.extensions.iter_mut().find(|e| e.name == name) {
                    Some(e) => *e = ext,
                    None => self.extensions.push(ext),
                }
            }
            "ext.loaded" => {
                let name = s("name");
                if let Some(e) = self.extensions.iter_mut().find(|e| e.name == name) {
                    e.loaded = true;
                }
            }
            "timer.start" => {
                let secs = a["seconds"].as_f64().unwrap_or(1.0).max(0.01);
                let every = a["repeat"].as_bool().unwrap_or(false).then(|| Duration::from_secs_f64(secs));
                self.timers.push(Timer {
                    id: a["id"].as_u64().unwrap_or(0),
                    proc_id: a["proc"].as_u64().unwrap_or(0),
                    at: Instant::now() + Duration::from_secs_f64(secs),
                    every,
                });
            }
            "timer.stop" => {
                let id = a["id"].as_u64().unwrap_or(0);
                self.timers.retain(|t| t.id != id);
            }
            _ => self.log(LineKind::Error, format!("Unknown notification from Ruby: {m}")),
        }
    }

    /// Turn an extension on (loads it now) or off (restarts Ruby without it).
    pub fn set_enabled(&mut self, doc: &mut Document, name: &str, enabled: bool) {
        self.settings.disabled.retain(|n| n != name);
        if !enabled {
            self.settings.disabled.push(name.to_string());
        }
        if let Err(e) = self.settings.save(&self.dirs) {
            self.log(LineKind::Error, format!("Could not save extension settings: {e}"));
        }
        if enabled {
            if let Some(e) = self.extensions.iter_mut().find(|e| e.name == name) {
                e.enabled = true;
            }
            // `check` loads it and reports `ext.loaded`.
            self.enqueue(Job::Eval(format!("Sketchup.extensions[{name:?}].check")));
        } else {
            self.restart(doc);
        }
    }

    /// Install a package and load it.
    pub fn install(&mut self, package: &std::path::Path) {
        match manager::install(&self.dirs, package) {
            Ok(loaders) => {
                let name = package.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.log(LineKind::Info, format!("Installed {name}"));
                self.notices.push((format!("Installed {name}"), false));
                for l in loaders {
                    self.enqueue(Job::Load(l));
                }
            }
            Err(e) => {
                self.log(LineKind::Error, e.clone());
                self.notices.push((e, true));
            }
        }
    }

    pub fn uninstall(&mut self, doc: &mut Document, name: &str) {
        let Some(ext) = self.extensions.iter().find(|e| e.name == name).cloned() else { return };
        match manager::uninstall(&self.dirs, ext.loader.as_deref(), &ext.path) {
            Ok(_) => {
                self.settings.disabled.retain(|n| n != name);
                let _ = self.settings.save(&self.dirs);
                self.notices.push((format!("Uninstalled {name}"), false));
                self.restart(doc);
            }
            Err(e) => self.notices.push((e, true)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: api::ViewInfo = api::ViewInfo { size: [800.0, 600.0] };

    fn pump_until(p: &mut Plugins, doc: &mut Document, what: &str, cond: impl Fn(&Plugins) -> bool) {
        let t0 = Instant::now();
        while !cond(p) {
            assert!(t0.elapsed() < Duration::from_secs(30), "timed out waiting for {what}; console:\n{:#?}", p.console);
            p.pump(doc, &VIEW, Duration::from_millis(20));
        }
    }

    fn idle(p: &Plugins) -> bool {
        !p.busy()
    }

    fn console_has(p: &Plugins, kind: LineKind, text: &str) -> bool {
        p.console.iter().any(|(k, l)| *k == kind && l.contains(text))
    }

    /// Install the Box Maker example (.rbz), run it from its menu through a real Ruby, answer its
    /// input box, then check the geometry, the undo step, the console and error reporting.
    #[test]
    fn box_maker_extension_end_to_end() {
        if host::find_ruby().is_none() {
            eprintln!("skipping: no Ruby interpreter");
            return;
        }
        let data = std::env::temp_dir().join(format!("skcad-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut p = Plugins::new(Dirs::new(data.clone()));
        let mut doc = Document::default();
        p.start();
        pump_until(&mut p, &mut doc, "start-up", |p| p.state == HostState::Ready);

        let rbz = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/box_maker.rbz");
        p.install(&rbz);
        pump_until(&mut p, &mut doc, "install", idle);
        let ext = p.extensions.iter().find(|e| e.name == "Box Maker").expect("registered");
        assert!(ext.loaded && ext.enabled && ext.version == "1.0.0", "{ext:?}");
        let create = p.menus.iter().find_map(|m| match m {
            MenuEntry::Item { menu, path, label, proc_id } if menu == "Extensions" && path == &["Box Maker"] && label == "Create Box..." => Some(*proc_id),
            _ => None,
        });
        let create = create.expect("menu item");
        let tb = p.toolbars.iter().find(|t| t.name == "Box Maker").expect("toolbar");
        assert!(tb.visible && tb.items[0].icon.as_ref().is_some_and(|i| i.is_file()));

        // Menu click → input box → answer it.
        p.enqueue(Job::Call { proc_id: create, label: "Create Box...".into() });
        pump_until(&mut p, &mut doc, "input box", |p| p.dialog.is_some());
        match &p.dialog {
            Some(Dialog::Input { prompts, values, .. }) => {
                assert_eq!(prompts, &["Width", "Depth", "Height"]);
                assert_eq!(values[0], "1000mm");
            }
            other => panic!("expected an input box, got {other:?}"),
        }
        p.answer_dialog(json!(["500mm", "400mm", "300mm"]));
        pump_until(&mut p, &mut doc, "box", idle);
        assert!(!p.console.iter().any(|(k, _)| *k == LineKind::Error), "{:#?}", p.console);

        // One group holding a closed 500 x 400 x 300 mm box, selected, as one undo step.
        assert_eq!(doc.model.mesh.instances.len(), 1);
        let inst = doc.model.mesh.instances.values().next().unwrap();
        assert_eq!(inst.name, "Box");
        let def = &doc.model.definitions[inst.def];
        assert!(def.is_group);
        assert_eq!(def.mesh.faces.len(), 6);
        let b = def.mesh.bounds();
        let mm = |v: f64| (v * 25.4).round();
        assert_eq!([mm(b.max.x - b.min.x), mm(b.max.y - b.min.y), mm(b.max.z - b.min.z)], [500.0, 400.0, 300.0]);
        assert!(b.min.z.abs() < 1e-6, "pulled up from the ground");
        assert_eq!(doc.selection.instances.len(), 1);
        assert_eq!(doc.history.undo_name(), Some("Create Box"));
        assert!(doc.undo());
        assert!(doc.model.mesh.instances.is_empty(), "undo removes the whole box");
        assert!(doc.redo());
        assert_eq!(doc.model.mesh.instances.len(), 1);

        // Ruby Console: results, puts output and errors with the failing line.
        p.enqueue(Job::Eval("Sketchup.active_model.entities.grep(Sketchup::Group).first.entities.grep(Sketchup::Face).map { |f| f.area.round }.sort".into()));
        p.enqueue(Job::Eval("puts 'hello from ruby'".into()));
        p.enqueue(Job::Eval("Sketchup.active_model.entities.add_face([0,0,0],[1,0,0])".into()));
        pump_until(&mut p, &mut doc, "console", idle);
        assert!(console_has(&p, LineKind::Result, "[186, 186, 233, 233, 310, 310]"), "{:#?}", p.console);
        pump_until(&mut p, &mut doc, "stderr", |p| console_has(p, LineKind::Output, "hello from ruby"));
        assert!(console_has(&p, LineKind::Error, "ArgumentError: add_face needs at least 3 points"), "{:#?}", p.console);

        // Turning the extension off reloads Ruby without it.
        p.set_enabled(&mut doc, "Box Maker", false);
        pump_until(&mut p, &mut doc, "restart", |p| p.state == HostState::Ready);
        let ext = p.extensions.iter().find(|e| e.name == "Box Maker").unwrap();
        assert!(!ext.enabled && !ext.loaded);
        assert!(p.menus.is_empty());
        p.uninstall(&mut doc, "Box Maker");
        pump_until(&mut p, &mut doc, "restart", |p| p.state == HostState::Ready);
        assert!(p.extensions.is_empty() && !p.dirs.plugins.join("box_maker.rb").exists());

        p.stop();
        std::fs::remove_dir_all(data).ok();
    }
}
