//! `.skp` → [`SkpModel`].

use crate::sdk::{Api, Pt, Ref, SU_ERROR_MODEL_VERSION, SU_ERROR_NONE, Tr, cstr, ok};
use sc_skp::*;
use std::collections::HashMap;
use std::path::Path;

struct Reader<'a> {
    api: &'a Api,
    model: SkpModel,
    materials: HashMap<usize, usize>,
    layers: HashMap<usize, Option<usize>>,
    definitions: HashMap<usize, usize>,
    /// Definitions being filled (placeholders until read).
    defs: Vec<Option<SkpDefinition>>,
    textured: usize,
    tagged_loose: usize,
    counts: [usize; 5],
}

pub fn read(api: &Api, path: &Path, lib_version: &str) -> Result<SkpModel, String> {
    let mut m = Ref::NULL;
    let r = unsafe { (api.SUModelCreateFromFile)(&mut m, cstr(&path.to_string_lossy()).as_ptr()) };
    if r == SU_ERROR_MODEL_VERSION {
        let file = file_version(path).map(|v| version_name(&v)).unwrap_or_else(|| "a newer SketchUp".into());
        return Err(format!(
            "This file was saved by {file}, but the SketchUp library in use ({}) can only open files from its own version or older.\nInstall the current SketchUp SDK (free from Trimble's developer site) and SketchCAD will use it.",
            version_name(lib_version)
        ));
    }
    ok(r, "Opening the SketchUp file")?;
    let mut rd = Reader { api, model: SkpModel::default(), materials: HashMap::new(), layers: HashMap::new(), definitions: HashMap::new(), defs: Vec::new(), textured: 0, tagged_loose: 0, counts: [0; 5] };
    let res = rd.read_model(m);
    unsafe { (api.SUModelRelease)(&mut m) };
    res?;
    let mut model = rd.model;
    model.definitions = rd.defs.into_iter().map(Option::unwrap_or_default).collect();
    let [dims, images, sections, guides, scenes] = rd.counts;
    let mut w = Vec::new();
    if rd.textured > 0 {
        w.push(format!("{} textured material(s) are shown as plain colors", rd.textured));
    }
    for (n, what) in [(scenes, "scene(s)"), (dims, "dimension(s)"), (images, "image(s)"), (sections, "section plane(s)"), (guides, "guide(s)")] {
        if n > 0 {
            w.push(format!("{n} {what} were not imported"));
        }
    }
    if rd.tagged_loose > 0 {
        w.push(format!("{} loose edges/faces on tags were imported untagged", rd.tagged_loose));
    }
    model.warnings = w;
    Ok(model)
}

impl Reader<'_> {
    fn read_model(&mut self, m: Ref) -> Result<(), String> {
        let api = self.api;
        unsafe {
            let (mut a, mut b, mut c) = (0, 0, 0);
            if (api.SUModelGetVersion)(m, &mut a, &mut b, &mut c) == SU_ERROR_NONE {
                self.model.version = format!("{a}.{b}.{c}");
            }
            let mut units = 0;
            if (api.SUModelGetUnits)(m, &mut units) == SU_ERROR_NONE {
                self.model.units = Some(["Inches", "Feet", "Millimeters", "Centimeters", "Meters"].get(units as usize).unwrap_or(&"Inches").to_string());
            }
            let mut cam = Ref::NULL;
            if (api.SUModelGetCamera)(m, &mut cam) == SU_ERROR_NONE {
                let (mut eye, mut target, mut up) = (Pt::default(), Pt::default(), Pt::default());
                let (mut persp, mut fov) = (true, 35.0);
                (api.SUCameraGetOrientation)(cam, &mut eye, &mut target, &mut up);
                (api.SUCameraGetPerspective)(cam, &mut persp);
                (api.SUCameraGetPerspectiveFrustumFOV)(cam, &mut fov);
                self.model.camera = Some(SkpCamera { eye: eye.arr(), target: target.arr(), up: up.arr(), perspective: persp, fov });
            }
            let mut n = 0usize;
            (api.SUModelGetNumScenes)(m, &mut n);
            self.counts[4] = n;
            let mut ents = Ref::NULL;
            ok((api.SUModelGetEntities)(m, &mut ents), "Reading the model")?;
            self.model.entities = self.entities(ents);
        }
        Ok(())
    }

    fn material(&mut self, mat: Ref) -> Option<usize> {
        if mat.is_null() {
            return None;
        }
        if let Some(i) = self.materials.get(&(mat.0 as usize)) {
            return Some(*i);
        }
        let api = self.api;
        let name = api.string(|s| unsafe { (api.SUMaterialGetName)(mat, s) });
        let mut c = crate::sdk::Col::default();
        unsafe {
            (api.SUMaterialGetColor)(mat, &mut c);
            let (mut use_opacity, mut opacity) = (false, 1.0);
            (api.SUMaterialGetUseOpacity)(mat, &mut use_opacity);
            (api.SUMaterialGetOpacity)(mat, &mut opacity);
            if use_opacity {
                c.a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            } else {
                c.a = 255;
            }
            let mut tex = Ref::NULL;
            if (api.SUMaterialGetTexture)(mat, &mut tex) == SU_ERROR_NONE && !tex.is_null() {
                self.textured += 1;
            }
        }
        self.model.materials.push(SkpMaterial { name, color: [c.r, c.g, c.b, c.a] });
        let i = self.model.materials.len() - 1;
        self.materials.insert(mat.0 as usize, i);
        Some(i)
    }

    fn layer(&mut self, elem: Ref) -> Option<usize> {
        let api = self.api;
        let mut l = Ref::NULL;
        if unsafe { (api.SUDrawingElementGetLayer)(elem, &mut l) } != SU_ERROR_NONE || l.is_null() {
            return None;
        }
        if let Some(i) = self.layers.get(&(l.0 as usize)) {
            return *i;
        }
        let name = api.string(|s| unsafe { (api.SULayerGetName)(l, s) });
        let idx = if name == "Layer0" || name == "Untagged" || name.is_empty() {
            None
        } else {
            let mut vis = true;
            unsafe { (api.SULayerGetVisibility)(l, &mut vis) };
            self.model.layers.push(SkpLayer { name, visible: vis });
            Some(self.model.layers.len() - 1)
        };
        self.layers.insert(l.0 as usize, idx);
        idx
    }

    fn loop_points(&self, lp: Ref) -> Vec<[f64; 3]> {
        let api = self.api;
        let verts = api.list(|n| unsafe { (api.SULoopGetNumVertices)(lp, n) }, |len, out, got| unsafe { (api.SULoopGetVertices)(lp, len, out, got) });
        verts
            .into_iter()
            .map(|v| {
                let mut p = Pt::default();
                unsafe { (api.SUVertexGetPosition)(v, &mut p) };
                p.arr()
            })
            .collect()
    }

    fn entities(&mut self, ents: Ref) -> SkpEntities {
        let api = self.api;
        let mut out = SkpEntities::default();
        unsafe {
            for (i, f) in [api.SUEntitiesGetNumDimensions, api.SUEntitiesGetNumImages, api.SUEntitiesGetNumSectionPlanes].into_iter().enumerate() {
                let mut n = 0usize;
                f(ents, &mut n);
                self.counts[i] += n;
            }
            let (mut gl, mut gp) = (0usize, 0usize);
            (api.SUEntitiesGetNumGuideLines)(ents, &mut gl);
            (api.SUEntitiesGetNumGuidePoints)(ents, &mut gp);
            self.counts[3] += gl + gp;

            let faces = api.list(|n| (api.SUEntitiesGetNumFaces)(ents, n), |len, o, g| (api.SUEntitiesGetFaces)(ents, len, o, g));
            for f in faces {
                let mut normal = Pt::default();
                (api.SUFaceGetNormal)(f, &mut normal);
                let mut outer = Ref::NULL;
                if (api.SUFaceGetOuterLoop)(f, &mut outer) != SU_ERROR_NONE {
                    continue;
                }
                let inner = api.list(|n| (api.SUFaceGetNumInnerLoops)(f, n), |len, o, g| (api.SUFaceGetInnerLoops)(f, len, o, g));
                let (mut fm, mut bm) = (Ref::NULL, Ref::NULL);
                (api.SUFaceGetFrontMaterial)(f, &mut fm);
                (api.SUFaceGetBackMaterial)(f, &mut bm);
                let face = SkpFace {
                    outer: self.loop_points(outer),
                    holes: inner.into_iter().map(|l| self.loop_points(l)).collect(),
                    normal: normal.arr(),
                    front: self.material(fm),
                    back: self.material(bm),
                };
                out.faces.push(face);
            }

            let edges = api.list(|n| (api.SUEntitiesGetNumEdges)(ents, false, n), |len, o, g| (api.SUEntitiesGetEdges)(ents, false, len, o, g));
            for e in edges {
                let (mut a, mut b) = (Ref::NULL, Ref::NULL);
                (api.SUEdgeGetStartVertex)(e, &mut a);
                (api.SUEdgeGetEndVertex)(e, &mut b);
                let (mut pa, mut pb) = (Pt::default(), Pt::default());
                (api.SUVertexGetPosition)(a, &mut pa);
                (api.SUVertexGetPosition)(b, &mut pb);
                let (mut soft, mut smooth, mut hidden) = (false, false, false);
                (api.SUEdgeGetSoft)(e, &mut soft);
                (api.SUEdgeGetSmooth)(e, &mut smooth);
                let el = (api.SUEdgeToDrawingElement)(e);
                (api.SUDrawingElementGetHidden)(el, &mut hidden);
                if self.layer(el).is_some() {
                    self.tagged_loose += 1;
                }
                let mut curve = Ref::NULL;
                let curve = ((api.SUEdgeGetCurve)(e, &mut curve) == SU_ERROR_NONE && !curve.is_null()).then_some(curve.0 as u64);
                out.edges.push(SkpEdge { a: pa.arr(), b: pb.arr(), soft, smooth, hidden, curve });
            }

            let groups = api.list(|n| (api.SUEntitiesGetNumGroups)(ents, n), |len, o, g| (api.SUEntitiesGetGroups)(ents, len, o, g));
            for g in groups {
                let name = api.string(|s| (api.SUGroupGetName)(g, s));
                let mut t = Tr { values: [0.0; 16] };
                (api.SUGroupGetTransform)(g, &mut t);
                let mut ge = Ref::NULL;
                if (api.SUGroupGetEntities)(g, &mut ge) != SU_ERROR_NONE {
                    continue;
                }
                // Each group gets its own definition (groups are unique by nature).
                let idx = self.defs.len();
                self.defs.push(None);
                let ents = self.entities(ge);
                self.defs[idx] = Some(SkpDefinition { name: if name.is_empty() { format!("Group#{}", idx + 1) } else { name.clone() }, description: String::new(), group: true, entities: ents });
                let el = (api.SUGroupToDrawingElement)(g);
                out.instances.push(self.instance(el, idx, t, name));
            }

            let insts = api.list(|n| (api.SUEntitiesGetNumInstances)(ents, n), |len, o, g| (api.SUEntitiesGetInstances)(ents, len, o, g));
            for i in insts {
                let mut d = Ref::NULL;
                if (api.SUComponentInstanceGetDefinition)(i, &mut d) != SU_ERROR_NONE {
                    continue;
                }
                let idx = self.definition(d);
                let mut t = Tr { values: [0.0; 16] };
                (api.SUComponentInstanceGetTransform)(i, &mut t);
                let name = api.string(|s| (api.SUComponentInstanceGetName)(i, s));
                let el = (api.SUComponentInstanceToDrawingElement)(i);
                out.instances.push(self.instance(el, idx, t, name));
            }
        }
        out
    }

    fn instance(&mut self, el: Ref, def: usize, t: Tr, name: String) -> SkpInstance {
        let api = self.api;
        let mut hidden = false;
        let mut mat = Ref::NULL;
        unsafe {
            (api.SUDrawingElementGetHidden)(el, &mut hidden);
            (api.SUDrawingElementGetMaterial)(el, &mut mat);
        }
        let layer = self.layer(el);
        let material = self.material(mat);
        SkpInstance { definition: def, transform: t.values, name, hidden, locked: false, layer, material }
    }

    fn definition(&mut self, d: Ref) -> usize {
        if let Some(i) = self.definitions.get(&(d.0 as usize)) {
            return *i;
        }
        let api = self.api;
        let idx = self.defs.len();
        self.defs.push(None);
        self.definitions.insert(d.0 as usize, idx);
        let name = api.string(|s| unsafe { (api.SUComponentDefinitionGetName)(d, s) });
        let description = api.string(|s| unsafe { (api.SUComponentDefinitionGetDescription)(d, s) });
        let mut ents = Ref::NULL;
        let entities = if unsafe { (api.SUComponentDefinitionGetEntities)(d, &mut ents) } == SU_ERROR_NONE { self.entities(ents) } else { SkpEntities::default() };
        self.defs[idx] = Some(SkpDefinition { name, description, group: false, entities });
        idx
    }
}
