//! [`SkpModel`] → `.skp`.

use crate::sdk::{Api, Col, MaterialInput, Pt, Ref, SU_ERROR_NONE, Tr, cstr, ok};
use sc_skp::*;
use std::collections::HashMap;
use std::path::Path;

struct Writer<'a> {
    api: &'a Api,
    src: &'a SkpModel,
    materials: Vec<Ref>,
    layers: Vec<Ref>,
    /// Component definitions (`None` for groups, which are created per instance).
    defs: Vec<Option<Ref>>,
}

pub fn write(api: &Api, src: &SkpModel, path: &Path) -> Result<(), String> {
    let mut m = Ref::NULL;
    ok(unsafe { (api.SUModelCreate)(&mut m) }, "Creating a SketchUp model")?;
    let mut w = Writer { api, src, materials: Vec::new(), layers: Vec::new(), defs: Vec::new() };
    let res = w.write_model(m).and_then(|_| ok(unsafe { (api.SUModelSaveToFile)(m, cstr(&path.to_string_lossy()).as_ptr()) }, "Saving the SketchUp file"));
    unsafe { (api.SUModelRelease)(&mut m) };
    res
}

impl Writer<'_> {
    fn write_model(&mut self, m: Ref) -> Result<(), String> {
        let api = self.api;
        let src = self.src;
        unsafe {
            if let Some(u) = &src.units {
                self.set_units(m, u);
            }
            if let Some(c) = &src.camera {
                let mut cam = Ref::NULL;
                if (api.SUModelGetCamera)(m, &mut cam) == SU_ERROR_NONE {
                    (api.SUCameraSetOrientation)(cam, &Pt::from(c.eye), &Pt::from(c.target), &Pt::from(c.up));
                    (api.SUCameraSetPerspective)(cam, c.perspective);
                    if c.perspective {
                        (api.SUCameraSetPerspectiveFrustumFOV)(cam, c.fov);
                    }
                }
            }

            for l in &src.layers {
                let mut r = Ref::NULL;
                ok((api.SULayerCreate)(&mut r), "Creating a tag")?;
                (api.SULayerSetName)(r, cstr(&l.name).as_ptr());
                (api.SULayerSetVisibility)(r, l.visible);
                self.layers.push(r);
            }
            if !self.layers.is_empty() {
                ok((api.SUModelAddLayers)(m, self.layers.len(), self.layers.as_ptr()), "Adding tags")?;
            }

            for mat in &src.materials {
                let mut r = Ref::NULL;
                ok((api.SUMaterialCreate)(&mut r), "Creating a material")?;
                (api.SUMaterialSetName)(r, cstr(&mat.name).as_ptr());
                let [red, green, blue, alpha] = mat.color;
                (api.SUMaterialSetColor)(r, &Col { r: red, g: green, b: blue, a: 255 });
                if alpha < 255 {
                    (api.SUMaterialSetUseOpacity)(r, true);
                    (api.SUMaterialSetOpacity)(r, alpha as f64 / 255.0);
                }
                self.materials.push(r);
            }
            if !self.materials.is_empty() {
                ok((api.SUModelAddMaterials)(m, self.materials.len(), self.materials.as_ptr()), "Adding materials")?;
            }

            // Component definitions first (instances anywhere can then refer to them).
            let mut comps = Vec::new();
            for d in &src.definitions {
                if d.group {
                    self.defs.push(None);
                    continue;
                }
                let mut r = Ref::NULL;
                ok((api.SUComponentDefinitionCreate)(&mut r), "Creating a component")?;
                (api.SUComponentDefinitionSetName)(r, cstr(&d.name).as_ptr());
                (api.SUComponentDefinitionSetDescription)(r, cstr(&d.description).as_ptr());
                self.defs.push(Some(r));
                comps.push(r);
            }
            if !comps.is_empty() {
                ok((api.SUModelAddComponentDefinitions)(m, comps.len(), comps.as_ptr()), "Adding components")?;
            }
            for (i, d) in src.definitions.iter().enumerate() {
                if let Some(r) = self.defs[i] {
                    let mut ents = Ref::NULL;
                    ok((api.SUComponentDefinitionGetEntities)(r, &mut ents), "Reading a component")?;
                    self.fill(ents, &d.entities)?;
                }
            }

            let mut ents = Ref::NULL;
            ok((api.SUModelGetEntities)(m, &mut ents), "Reading the model")?;
            self.fill(ents, &src.entities)?;
        }
        Ok(())
    }

    /// Units and length format (`UnitsOptions`), best effort.
    unsafe fn set_units(&self, m: Ref, units: &str) {
        let api = self.api;
        let (unit, format) = match units {
            "Architectural" => (0, 1),
            "Inches" => (0, 0),
            "Feet" => (1, 0),
            "Millimeters" => (2, 0),
            "Centimeters" => (3, 0),
            "Meters" => (4, 0),
            _ => return,
        };
        unsafe {
            let (mut om, mut prov) = (Ref::NULL, Ref::NULL);
            if (api.SUModelGetOptionsManager)(m, &mut om) != SU_ERROR_NONE || (api.SUOptionsManagerGetOptionsProviderByName)(om, cstr("UnitsOptions").as_ptr(), &mut prov) != SU_ERROR_NONE {
                return;
            }
            for (key, v) in [("LengthFormat", format), ("LengthUnit", unit)] {
                let mut tv = Ref::NULL;
                if (api.SUTypedValueCreate)(&mut tv) == SU_ERROR_NONE {
                    (api.SUTypedValueSetInt32)(tv, v);
                    (api.SUOptionsProviderSetValue)(prov, cstr(key).as_ptr(), tv);
                    (api.SUTypedValueRelease)(&mut tv);
                }
            }
        }
    }

    fn material_input(&self, idx: Option<usize>) -> Option<MaterialInput> {
        let material = *self.materials.get(idx?)?;
        Some(MaterialInput { num_uv_coords: 0, uv_coords: [[0.0; 2]; 4], vertex_indices: [0; 4], material })
    }

    /// Put `src`'s faces, edges and instances into `ents`.
    fn fill(&mut self, ents: Ref, src: &SkpEntities) -> Result<(), String> {
        let api = self.api;
        unsafe {
            if !src.faces.is_empty() || !src.edges.is_empty() {
                // Shared vertex list; edge flags looked up by vertex pair.
                let mut index: HashMap<[u64; 3], usize> = HashMap::new();
                let mut verts: Vec<Pt> = Vec::new();
                let mut vid = |p: [f64; 3]| -> usize {
                    *index.entry([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]).or_insert_with(|| {
                        verts.push(Pt::from(p));
                        verts.len() - 1
                    })
                };
                let key = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
                let mut flags: HashMap<(usize, usize), (bool, bool, bool)> = HashMap::new();
                let mut edges: Vec<(usize, usize)> = Vec::new();
                for e in &src.edges {
                    let (a, b) = (vid(e.a), vid(e.b));
                    if a != b {
                        flags.insert(key(a, b), (e.soft, e.smooth, e.hidden));
                        edges.push((a, b));
                    }
                }
                let loops: Vec<(Vec<usize>, Vec<Vec<usize>>)> = src
                    .faces
                    .iter()
                    .map(|f| (f.outer.iter().map(|p| vid(*p)).collect(), f.holes.iter().map(|h| h.iter().map(|p| vid(*p)).collect()).collect()))
                    .collect();

                let mut geom = Ref::NULL;
                ok((api.SUGeometryInputCreate)(&mut geom), "Preparing geometry")?;
                (api.SUGeometryInputSetVertices)(geom, verts.len(), verts.as_ptr());
                let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
                let make_loop = |l: &[usize], used: &mut std::collections::HashSet<(usize, usize)>| -> Ref {
                    let mut li = Ref::NULL;
                    (api.SULoopInputCreate)(&mut li);
                    for (i, v) in l.iter().enumerate() {
                        (api.SULoopInputAddVertexIndex)(li, *v);
                        let k = key(*v, l[(i + 1) % l.len()]);
                        used.insert(k);
                        if let Some((soft, smooth, hidden)) = flags.get(&k) {
                            (api.SULoopInputEdgeSetSoft)(li, i, *soft);
                            (api.SULoopInputEdgeSetSmooth)(li, i, *smooth);
                            (api.SULoopInputEdgeSetHidden)(li, i, *hidden);
                        }
                    }
                    li
                };
                for (f, (outer, holes)) in src.faces.iter().zip(&loops) {
                    if outer.len() < 3 {
                        continue;
                    }
                    let mut li = make_loop(outer, &mut used);
                    let mut fi = 0usize;
                    if (api.SUGeometryInputAddFace)(geom, &mut li, &mut fi) != SU_ERROR_NONE {
                        if !li.is_null() {
                            (api.SULoopInputRelease)(&mut li);
                        }
                        continue;
                    }
                    for h in holes.iter().filter(|h| h.len() >= 3) {
                        let mut hl = make_loop(h, &mut used);
                        if (api.SUGeometryInputFaceAddInnerLoop)(geom, fi, &mut hl) != SU_ERROR_NONE && !hl.is_null() {
                            (api.SULoopInputRelease)(&mut hl);
                        }
                    }
                    if let Some(mi) = self.material_input(f.front) {
                        (api.SUGeometryInputFaceSetFrontMaterial)(geom, fi, &mi);
                    }
                    if let Some(mi) = self.material_input(f.back) {
                        (api.SUGeometryInputFaceSetBackMaterial)(geom, fi, &mi);
                    }
                }
                // Edges not bounding any face.
                for (a, b) in edges {
                    if used.contains(&key(a, b)) {
                        continue;
                    }
                    let mut ei = 0usize;
                    if (api.SUGeometryInputAddEdge)(geom, a, b, &mut ei) == SU_ERROR_NONE
                        && let Some((soft, smooth, hidden)) = flags.get(&key(a, b)) {
                            (api.SUGeometryInputEdgeSetSoft)(geom, ei, *soft);
                            (api.SUGeometryInputEdgeSetSmooth)(geom, ei, *smooth);
                            (api.SUGeometryInputEdgeSetHidden)(geom, ei, *hidden);
                        }
                }
                let r = (api.SUEntitiesFill)(ents, geom, true);
                (api.SUGeometryInputRelease)(&mut geom);
                ok(r, "Adding geometry")?;
            }

            for inst in &src.instances {
                let Some(def) = self.src.definitions.get(inst.definition) else { continue };
                let t = Tr { values: inst.transform };
                let el = if def.group {
                    let mut g = Ref::NULL;
                    ok((api.SUGroupCreate)(&mut g), "Creating a group")?;
                    ok((api.SUEntitiesAddGroup)(ents, g), "Adding a group")?;
                    (api.SUGroupSetTransform)(g, &t);
                    if !inst.name.is_empty() {
                        (api.SUGroupSetName)(g, cstr(&inst.name).as_ptr());
                    }
                    let mut ge = Ref::NULL;
                    ok((api.SUGroupGetEntities)(g, &mut ge), "Reading a group")?;
                    self.fill(ge, &def.entities)?;
                    (api.SUGroupToDrawingElement)(g)
                } else {
                    let Some(Some(d)) = self.defs.get(inst.definition).copied() else { continue };
                    let mut ci = Ref::NULL;
                    ok((api.SUComponentDefinitionCreateInstance)(d, &mut ci), "Placing a component")?;
                    (api.SUComponentInstanceSetTransform)(ci, &t);
                    if !inst.name.is_empty() {
                        (api.SUComponentInstanceSetName)(ci, cstr(&inst.name).as_ptr());
                    }
                    ok((api.SUEntitiesAddInstance)(ents, ci, std::ptr::null_mut()), "Adding a component")?;
                    if inst.locked {
                        (api.SUComponentInstanceSetLocked)(ci, true);
                    }
                    (api.SUComponentInstanceToDrawingElement)(ci)
                };
                if inst.hidden {
                    (api.SUDrawingElementSetHidden)(el, true);
                }
                if let Some(l) = inst.layer.and_then(|i| self.layers.get(i)) {
                    (api.SUDrawingElementSetLayer)(el, *l);
                }
                if let Some(m) = inst.material.and_then(|i| self.materials.get(i)) {
                    (api.SUDrawingElementSetMaterial)(el, *m);
                }
            }
        }
        Ok(())
    }
}
