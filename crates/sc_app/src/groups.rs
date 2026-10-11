//! Groups and components: make, explode, make unique.

use crate::app::SketchCadApp;
use crate::document::Definition;
use sc_geom::Instance;
use sc_math::{DMat4, DVec3};
use std::collections::HashSet;

impl SketchCadApp {
    /// Turn the selection into a new group (`is_group`) or component instance.
    pub fn make_definition(&mut self, is_group: bool, name: Option<String>, description: String) {
        if self.doc.selection.is_empty() {
            return;
        }
        self.with_tool_cx(|t, cx| t.cancel(cx));
        self.doc.checkpoint(if is_group { "Make Group" } else { "Make Component" });
        let sel = std::mem::take(&mut self.doc.selection);
        let name = name.unwrap_or_else(|| self.doc.model.unique_def_name(if is_group { "Group" } else { "Component" }));
        let edges: Vec<_> = sel.edges.into_iter().collect();
        let faces: Vec<_> = sel.faces.into_iter().collect();
        let insts: Vec<_> = sel.instances.into_iter().collect();
        let mut sub = self.doc.active_mesh_mut().extract(&edges, &faces, &insts);

        // Components get their axes at the bounding-box corner (like SketchUp); groups keep the parent's.
        let xform = if is_group {
            DMat4::IDENTITY
        } else {
            let mut b = sub.bounds();
            for i in sub.instances.values() {
                let ib = self.doc.model.instance_bounds(i);
                if !ib.is_empty() {
                    b.extend(ib.min);
                    b.extend(ib.max);
                }
            }
            let origin = if b.is_empty() { DVec3::ZERO } else { b.min };
            let all: HashSet<_> = sub.vertices.keys().collect();
            sub.move_vertices(&all, -origin);
            let shift = DMat4::from_translation(-origin);
            for i in sub.instances.values_mut() {
                i.xform = shift * i.xform;
            }
            DMat4::from_translation(origin)
        };
        let def = self.doc.model.definitions.insert(Definition { name, description, is_group, mesh: sub });
        let m = self.doc.active_mesh_mut();
        let id = m.instances.insert(Instance::new(def, xform));
        m.revision += 1;
        self.doc.selection.instances.insert(id);
    }

    /// Replace selected groups/components with their geometry.
    pub fn explode_selection(&mut self) {
        if self.doc.selection.instances.is_empty() {
            return;
        }
        self.with_tool_cx(|t, cx| t.cancel(cx));
        self.doc.checkpoint("Explode");
        let insts: Vec<_> = self.doc.selection.instances.drain().collect();
        for id in insts {
            let Some(inst) = self.doc.active_mesh_mut().instances.remove(id) else { continue };
            let Some(def_mesh) = self.doc.model.definitions.get(inst.def).map(|d| d.mesh.clone()) else { continue };
            let (edges, faces, instances) = self.doc.active_mesh_mut().merge_mesh(&def_mesh, inst.xform);
            let sel = &mut self.doc.selection;
            sel.edges.extend(edges);
            sel.faces.extend(faces);
            sel.instances.extend(instances);
        }
    }

    /// Give each selected component its own copy of the definition.
    pub fn make_unique(&mut self) {
        let insts: Vec<_> = self.doc.selection.instances.iter().copied().collect();
        if insts.is_empty() {
            return;
        }
        self.doc.checkpoint("Make Unique");
        for id in insts {
            let def = self.doc.active_mesh().instances[id].def;
            if self.doc.model.instance_count(def) < 2 {
                continue;
            }
            let mut copy = self.doc.model.definitions[def].clone();
            copy.name = self.doc.model.unique_def_name(copy.name.split('#').next().unwrap_or("Component"));
            let new_def = self.doc.model.definitions.insert(copy);
            let m = self.doc.active_mesh_mut();
            m.instances[id].def = new_def;
            m.revision += 1;
        }
    }
}
