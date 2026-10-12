//! The open model: geometry, components, tags, scenes, view settings, selection, edit context
//! and undo history.

use sc_geom::{DefId, EdgeId, FaceId, InstanceId, Mesh, TexId, Texture, VertexId};
use sc_math::units::UnitSettings;
use sc_math::{Aabb, DMat4, DVec3};
use sc_render::{Camera, FaceStyle};
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const FILE_EXTENSION: &str = "skcad";
const FORMAT_TAG: &str = "SketchCAD";
const FORMAT_VERSION: u32 = 2;

/// Construction geometry created by the Tape Measure tool (world space).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Guide {
    Line { point: DVec3, dir: DVec3 },
    Point { point: DVec3 },
}

impl Guide {
    pub fn transformed(&self, m: &DMat4) -> Guide {
        match *self {
            Guide::Line { point, dir } => Guide::Line { point: m.transform_point3(point), dir: m.transform_vector3(dir).normalize() },
            Guide::Point { point } => Guide::Point { point: m.transform_point3(point) },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleSettings {
    pub face_style: FaceStyle,
    pub back_edges: bool,
    pub show_axes: bool,
    pub show_guides: bool,
    pub show_sky_ground: bool,
}

impl Default for StyleSettings {
    fn default() -> Self {
        Self { face_style: FaceStyle::ShadedWithTextures, back_edges: false, show_axes: true, show_guides: true, show_sky_ground: true }
    }
}

/// A group or component definition. Groups are definitions with a single owner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub description: String,
    pub is_group: bool,
    pub mesh: Mesh,
}

/// A tag (formerly "layer"): controls visibility of the groups/components assigned to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub visible: bool,
    pub color: [u8; 4],
}

/// A saved view: camera, style and tag visibility.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub camera: Camera,
    pub style: StyleSettings,
    pub hidden_tags: Vec<u32>,
}

/// Everything persisted in a `.skcad` file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelData {
    /// Top-level ("model") context.
    pub mesh: Mesh,
    #[serde(default)]
    pub definitions: SlotMap<DefId, Definition>,
    /// Images painted on faces, shared by every mesh in the model.
    #[serde(default)]
    pub textures: SlotMap<TexId, Texture>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    pub guides: Vec<Guide>,
    pub camera: Camera,
    pub units: UnitSettings,
    pub style: StyleSettings,
}

impl Default for ModelData {
    fn default() -> Self {
        Self {
            mesh: Mesh::new(),
            definitions: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            tags: Vec::new(),
            scenes: Vec::new(),
            guides: Vec::new(),
            camera: Camera::default(),
            units: UnitSettings::default(),
            style: StyleSettings::default(),
        }
    }
}

impl ModelData {
    /// The mesh of a context: `None` is the model root.
    pub fn context_mesh(&self, ctx: Option<DefId>) -> &Mesh {
        match ctx {
            None => &self.mesh,
            Some(d) => &self.definitions[d].mesh,
        }
    }

    pub fn context_mesh_mut(&mut self, ctx: Option<DefId>) -> &mut Mesh {
        match ctx {
            None => &mut self.mesh,
            Some(d) => &mut self.definitions[d].mesh,
        }
    }

    pub fn tag_visible(&self, tag: Option<u32>) -> bool {
        tag.and_then(|t| self.tags.get(t as usize)).is_none_or(|t| t.visible)
    }

    pub fn instance_visible(&self, i: &sc_geom::Instance) -> bool {
        !i.hidden && self.tag_visible(i.tag)
    }

    /// Number of instances of each definition, across all contexts.
    pub fn instance_count(&self, def: DefId) -> usize {
        let count = |m: &Mesh| m.instances.values().filter(|i| i.def == def).count();
        count(&self.mesh) + self.definitions.values().map(|d| count(&d.mesh)).sum::<usize>()
    }

    /// Bounding box of an instance's geometry in its parent's space.
    pub fn instance_bounds(&self, inst: &sc_geom::Instance) -> Aabb {
        let mut b = Aabb::EMPTY;
        self.extend_bounds(inst.def, &inst.xform, &mut b);
        b
    }

    /// Bounding box of a definition in its own coordinates (including nested instances).
    pub fn def_bounds(&self, def: DefId) -> Aabb {
        let mut b = Aabb::EMPTY;
        self.extend_bounds(def, &DMat4::IDENTITY, &mut b);
        b
    }

    fn extend_bounds(&self, def: DefId, xf: &DMat4, b: &mut Aabb) {
        let Some(d) = self.definitions.get(def) else { return };
        for v in d.mesh.vertices.values() {
            b.extend(xf.transform_point3(v.pos));
        }
        for i in d.mesh.instances.values() {
            self.extend_bounds(i.def, &(*xf * i.xform), b);
        }
    }

    /// All visible geometry of the model in world space.
    pub fn bounds(&self) -> Aabb {
        let mut b = self.mesh.bounds();
        for i in self.mesh.instances.values() {
            if self.instance_visible(i) {
                self.extend_bounds(i.def, &i.xform, &mut b);
            }
        }
        b
    }

    /// Append a flattened, transformed copy of a definition (and its visible nested instances).
    pub fn flatten_into(&self, def: DefId, xf: &DMat4, out: &mut Mesh) {
        let Some(d) = self.definitions.get(def) else { return };
        out.append_raw(&d.mesh, xf);
        for i in d.mesh.instances.values() {
            if self.instance_visible(i) {
                self.flatten_into(i.def, &(*xf * i.xform), out);
            }
        }
    }

    /// Unique name like "Group#3" / "Component#2".
    pub fn unique_def_name(&self, base: &str) -> String {
        (1..).map(|n| format!("{base}#{n}")).find(|n| !self.definitions.values().any(|d| &d.name == n)).unwrap()
    }

    /// Definition ids reachable from the root (anything else is an orphan, e.g. after undo).
    pub fn used_definitions(&self) -> HashSet<DefId> {
        let mut used = HashSet::new();
        let mut stack: Vec<DefId> = self.mesh.instances.values().map(|i| i.def).collect();
        while let Some(d) = stack.pop() {
            if used.insert(d)
                && let Some(def) = self.definitions.get(d) {
                    stack.extend(def.mesh.instances.values().map(|i| i.def));
                }
        }
        used
    }
}

#[derive(Serialize, Deserialize)]
struct FileEnvelope {
    format: String,
    version: u32,
    model: ModelData,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub edges: HashSet<EdgeId>,
    pub faces: HashSet<FaceId>,
    pub instances: HashSet<InstanceId>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.faces.is_empty() && self.instances.is_empty()
    }

    pub fn clear(&mut self) {
        self.edges.clear();
        self.faces.clear();
        self.instances.clear();
    }

    pub fn len(&self) -> usize {
        self.edges.len() + self.faces.len() + self.instances.len()
    }

    /// Drop ids that no longer exist in `mesh`.
    pub fn prune(&mut self, mesh: &Mesh) {
        self.edges.retain(|e| mesh.edges.contains_key(*e));
        self.faces.retain(|f| mesh.faces.contains_key(*f));
        self.instances.retain(|i| mesh.instances.contains_key(*i));
    }

    /// Every vertex touched by the selection.
    pub fn vertices(&self, mesh: &Mesh) -> HashSet<VertexId> {
        let mut out = HashSet::new();
        for e in &self.edges {
            if let Some(ed) = mesh.edges.get(*e) {
                out.extend(ed.v);
            }
        }
        for f in &self.faces {
            if let Some(face) = mesh.faces.get(*f) {
                out.extend(face.vertex_ids());
            }
        }
        out
    }

    /// Add the edges bounding selected faces (what moving a face actually moves).
    pub fn with_bounding_edges(&self, mesh: &Mesh) -> Selection {
        let mut s = self.clone();
        for f in &self.faces {
            if let Some(face) = mesh.faces.get(*f) {
                for (a, b) in face.loop_pairs() {
                    if let Some(e) = mesh.edge_between(a, b) {
                        s.edges.insert(e);
                    }
                }
            }
        }
        s
    }
}

pub struct UndoEntry {
    pub name: String,
    /// Context the snapshot belongs to (`None` = model root).
    pub ctx: Option<DefId>,
    pub mesh: Mesh,
    pub guides: Vec<Guide>,
    /// Every definition, for operations that can touch several contexts (Ruby extensions).
    pub defs: Option<SlotMap<DefId, Definition>>,
}

/// Snapshot-based undo of one context per step. Simple and always correct; a diff journal replaces
/// it once models get large.
#[derive(Default)]
pub struct History {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl History {
    const LIMIT: usize = 100;

    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|e| e.name.as_str())
    }

    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|e| e.name.as_str())
    }

    pub fn push(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
        if self.undo.len() > Self::LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

#[derive(Default)]
pub struct Document {
    pub model: ModelData,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub selection: Selection,
    pub history: History,
    /// Open group/component edits, outermost first.
    pub edit_path: Vec<InstanceId>,
}

impl Document {
    pub fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        format!("{}{} - SketchCAD", if self.dirty { "*" } else { "" }, name)
    }

    /// Resolve the edit path to (instance, definition) pairs, truncating it if it went stale.
    pub fn edit_chain(&mut self) -> Vec<(InstanceId, DefId)> {
        let mut chain = Vec::new();
        let mut ctx: Option<DefId> = None;
        for &inst in &self.edit_path {
            match self.model.context_mesh(ctx).instances.get(inst) {
                Some(i) if self.model.definitions.contains_key(i.def) => {
                    chain.push((inst, i.def));
                    ctx = Some(i.def);
                }
                _ => break,
            }
        }
        self.edit_path.truncate(chain.len());
        chain
    }

    /// The context being edited: `None` = model root.
    pub fn active_ctx(&self) -> Option<DefId> {
        let mut ctx: Option<DefId> = None;
        for &inst in &self.edit_path {
            match self.model.context_mesh(ctx).instances.get(inst) {
                Some(i) => ctx = Some(i.def),
                None => break,
            }
        }
        ctx
    }

    /// Active context space → world space.
    pub fn active_xform(&self) -> DMat4 {
        let mut ctx: Option<DefId> = None;
        let mut xf = DMat4::IDENTITY;
        for &inst in &self.edit_path {
            match self.model.context_mesh(ctx).instances.get(inst) {
                Some(i) => {
                    xf *= i.xform;
                    ctx = Some(i.def);
                }
                None => break,
            }
        }
        xf
    }

    pub fn active_mesh(&self) -> &Mesh {
        self.model.context_mesh(self.active_ctx())
    }

    pub fn active_mesh_mut(&mut self) -> &mut Mesh {
        let ctx = self.active_ctx();
        self.model.context_mesh_mut(ctx)
    }

    /// Record the active context before an operation named `name` (shown as "Undo <name>").
    pub fn checkpoint(&mut self, name: &str) {
        let mesh = self.active_mesh().clone();
        self.checkpoint_with(name, mesh);
    }

    pub fn checkpoint_with(&mut self, name: &str, before: Mesh) {
        let ctx = self.active_ctx();
        self.history.push(UndoEntry { name: name.to_string(), ctx, mesh: before, guides: self.model.guides.clone(), defs: None });
        self.dirty = true;
    }

    /// Snapshot of the model root and every definition, taken before an operation that may touch
    /// any context. Record it with [`Document::push_model_snapshot`] once the operation is done.
    pub fn model_snapshot(&self) -> UndoEntry {
        UndoEntry { name: String::new(), ctx: None, mesh: self.model.mesh.clone(), guides: self.model.guides.clone(), defs: Some(self.model.definitions.clone()) }
    }

    pub fn push_model_snapshot(&mut self, name: &str, mut snap: UndoEntry) {
        snap.name = name.to_string();
        self.history.push(snap);
        self.dirty = true;
    }

    /// Put the model back to a snapshot without recording a redo step (aborted operations).
    pub fn restore_snapshot(&mut self, snap: UndoEntry) {
        self.swap_entry(snap);
        self.after_history_change();
    }

    fn swap_entry(&mut self, e: UndoEntry) -> Option<UndoEntry> {
        if e.defs.is_none() && e.ctx.is_some_and(|d| !self.model.definitions.contains_key(d)) {
            return None;
        }
        let slot = self.model.context_mesh_mut(e.ctx);
        let mesh = std::mem::replace(slot, e.mesh);
        slot.revision = slot.revision.max(mesh.revision) + 1;
        let guides = std::mem::replace(&mut self.model.guides, e.guides);
        let defs = e.defs.map(|mut d| {
            // Keep render caches keyed on revisions moving forward.
            for (id, def) in d.iter_mut() {
                if let Some(cur) = self.model.definitions.get(id) {
                    def.mesh.revision = def.mesh.revision.max(cur.mesh.revision) + 1;
                }
            }
            std::mem::replace(&mut self.model.definitions, d)
        });
        Some(UndoEntry { name: e.name, ctx: e.ctx, mesh, guides, defs })
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.history.undo.pop() else { return false };
        if let Some(cur) = self.swap_entry(e) {
            self.history.redo.push(cur);
        }
        self.after_history_change();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.history.redo.pop() else { return false };
        if let Some(cur) = self.swap_entry(e) {
            self.history.undo.push(cur);
        }
        self.after_history_change();
        true
    }

    fn after_history_change(&mut self) {
        self.edit_chain();
        let ctx = self.active_ctx();
        self.selection.prune(self.model.context_mesh(ctx));
        self.dirty = true;
    }

    /// Open a group/component for editing (it must be in the active context).
    /// Editing one copy of a group that shares its definition makes that copy unique first.
    pub fn enter(&mut self, inst: InstanceId) {
        let ctx = self.active_ctx();
        let Some(i) = self.model.context_mesh(ctx).instances.get(inst) else { return };
        let def = i.def;
        if self.model.definitions[def].is_group && self.model.instance_count(def) > 1 {
            let copy = self.model.definitions[def].clone();
            let new_def = self.model.definitions.insert(copy);
            self.model.context_mesh_mut(ctx).instances[inst].def = new_def;
        }
        self.edit_path.push(inst);
        self.selection.clear();
    }

    /// Close the innermost open group/component, selecting it.
    pub fn exit(&mut self) -> bool {
        let Some(inst) = self.edit_path.pop() else { return false };
        self.selection.clear();
        self.selection.instances.insert(inst);
        true
    }

    /// Revision fingerprint of every context, for render caches.
    pub fn revision_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.model.mesh.revision.hash(&mut h);
        for (id, d) in &self.model.definitions {
            id.hash(&mut h);
            d.mesh.revision.hash(&mut h);
        }
        for t in &self.model.tags {
            t.visible.hash(&mut h);
        }
        self.edit_path.hash(&mut h);
        h.finish()
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let env = FileEnvelope { format: FORMAT_TAG.into(), version: FORMAT_VERSION, model: self.model.clone() };
        let json = serde_json::to_vec(&env).map_err(|e| e.to_string())?;
        // Write atomically so a crash mid-save never destroys the previous file.
        let tmp = path.with_extension(format!("{FILE_EXTENSION}~"));
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Document, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let env: FileEnvelope = serde_json::from_slice(&bytes).map_err(|e| format!("Not a valid SketchCAD file: {e}"))?;
        if env.format != FORMAT_TAG {
            return Err("Not a SketchCAD file".into());
        }
        if env.version > FORMAT_VERSION {
            return Err(format!("File was saved by a newer SketchCAD (format v{})", env.version));
        }
        let mut model = env.model;
        model.mesh.rebuild_links();
        for d in model.definitions.values_mut() {
            d.mesh.rebuild_links();
        }
        Ok(Document { model, path: Some(path.to_path_buf()), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_files_still_load() {
        // A v1 file had no definitions, tags, scenes or instances.
        let mut model = ModelData::default();
        model.mesh.add_polyline(&[DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0)], true, false);
        let mut v = serde_json::to_value(FileEnvelope { format: FORMAT_TAG.into(), version: 1, model }).unwrap();
        let m = v["model"].as_object_mut().unwrap();
        for k in ["definitions", "tags", "scenes"] {
            m.remove(k);
        }
        m["mesh"].as_object_mut().unwrap().remove("instances");
        let dir = std::env::temp_dir().join(format!("skcad-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.skcad");
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        let doc = Document::load(&path).unwrap();
        assert_eq!(doc.model.mesh.faces.len(), 1);
        assert!(doc.model.definitions.is_empty() && doc.model.tags.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
