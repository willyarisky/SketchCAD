//! `.skp` → [`SkpModel`].

use crate::sdk::{Api, Pt, Ref, SU_ERROR_MODEL_VERSION, SU_ERROR_NONE, Tr, cstr, ok};
use sc_skp::*;
use std::collections::HashMap;
use std::path::Path;

struct Reader<'a> {
    api: &'a Api,
    model: SkpModel,
    materials: HashMap<usize, usize>,
    textures: HashMap<usize, Option<usize>>,
    layers: HashMap<usize, Option<usize>>,
    definitions: HashMap<usize, usize>,
    /// Definitions being filled (placeholders until read).
    defs: Vec<Option<SkpDefinition>>,
    /// Where texture images are written: `<stem>-tex<N>.<ext>` beside the output JSON.
    tex_stem: std::path::PathBuf,
    /// Textured materials whose image could not be unpacked.
    textured: usize,
    /// Faces whose texture is stretched in perspective, which an affine map cannot express.
    distorted: usize,
    tagged_loose: usize,
    counts: [usize; 5],
}

pub fn read(api: &Api, path: &Path, out_json: &Path, lib_version: &str) -> Result<SkpModel, String> {
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
    let tex_stem = out_json.with_extension("");
    let mut rd = Reader {
        api,
        model: SkpModel::default(),
        materials: HashMap::new(),
        textures: HashMap::new(),
        layers: HashMap::new(),
        definitions: HashMap::new(),
        defs: Vec::new(),
        tex_stem,
        textured: 0,
        distorted: 0,
        tagged_loose: 0,
        counts: [0; 5],
    };
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
    if rd.distorted > 0 {
        w.push(format!("{} face(s) have a texture stretched in perspective, shown evenly tiled", rd.distorted));
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

type UvQ = unsafe extern "C" fn(Ref, *const Pt, *mut Pt) -> std::ffi::c_int;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn norm(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-12).then(|| scale(a, 1.0 / l))
}

/// Three points of `pts` spanning as much of the loop as possible, so solving against them is
/// numerically stable. `None` if the points are collinear.
fn spread_triangle(pts: &[[f64; 3]]) -> Option<[[f64; 3]; 3]> {
    let a = *pts.first()?;
    let d2 = |p: [f64; 3]| dot(sub(p, a), sub(p, a));
    let b = *pts.iter().max_by(|p, q| d2(**p).total_cmp(&d2(**q)))?;
    if d2(b) < 1e-18 {
        return None;
    }
    let ab = norm(sub(b, a))?;
    // Farthest from the line a-b, i.e. the largest triangle area.
    let h2 = |p: [f64; 3]| dot(cross(sub(p, a), ab), cross(sub(p, a), ab));
    let c = *pts.iter().max_by(|p, q| h2(**p).total_cmp(&h2(**q)))?;
    (h2(c) > 1e-18).then_some([a, b, c])
}

/// Fit `u = dot(p, u_axis) + u_offset` (and the same for v) through three samples.
///
/// The three points are coplanar, so in 3D the fit is underdetermined; constraining the axes to
/// the face's plane — where the texture actually lives — makes it exactly determined.
fn affine_uv(p: [[f64; 3]; 3], uv: [[f64; 2]; 3], normal: [f64; 3]) -> Option<SkpUv> {
    let n = norm(normal)?;
    let o = p[0];
    let e1 = norm(sub(p[1], o))?;
    let e2 = norm(cross(n, e1))?;
    // Plane coordinates of the samples; the first is the origin, so its row is (0, 0, 1).
    let st = |q: [f64; 3]| [dot(sub(q, o), e1), dot(sub(q, o), e2)];
    let (s1, s2) = (st(p[1]), st(p[2]));
    // Solve [0 0 1; s1 t1 1; s2 t2 1] * [A B C]^T = channel, by elimination on the 2x2 left block.
    let det = s1[0] * s2[1] - s1[1] * s2[0];
    if det.abs() < 1e-18 {
        return None;
    }
    let axis = |c: [f64; 3]| -> [f64; 4] {
        let cc = c[0]; // value at the origin sample
        let (r1, r2) = (c[1] - cc, c[2] - cc);
        let a = (r1 * s2[1] - r2 * s1[1]) / det;
        let b = (s1[0] * r2 - s2[0] * r1) / det;
        // Back to model space: u = dot(p - o, a*e1 + b*e2) + cc.
        let ax = [a * e1[0] + b * e2[0], a * e1[1] + b * e2[1], a * e1[2] + b * e2[2]];
        [ax[0], ax[1], ax[2], cc - dot(o, ax)]
    };
    Some(SkpUv { u: axis([uv[0][0], uv[1][0], uv[2][0]]), v: axis([uv[0][1], uv[1][1], uv[2][1]]) })
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
        }
        let mut tex = Ref::NULL;
        let texture = if unsafe { (api.SUMaterialGetTexture)(mat, &mut tex) } == SU_ERROR_NONE && !tex.is_null() {
            let t = self.texture(tex);
            if t.is_none() {
                self.textured += 1;
            }
            t
        } else {
            None
        };
        self.model.materials.push(SkpMaterial { name, color: [c.r, c.g, c.b, c.a], texture });
        let i = self.model.materials.len() - 1;
        self.materials.insert(mat.0 as usize, i);
        Some(i)
    }

    /// Unpack a texture's image into a file beside the output JSON. `None` if it cannot be read,
    /// in which case the material falls back to its average colour.
    fn texture(&mut self, tex: Ref) -> Option<usize> {
        if let Some(i) = self.textures.get(&(tex.0 as usize)) {
            return *i;
        }
        let idx = self.unpack_texture(tex);
        self.textures.insert(tex.0 as usize, idx);
        idx
    }

    fn unpack_texture(&mut self, tex: Ref) -> Option<usize> {
        let api = self.api;
        let (mut w, mut h, mut s_scale, mut t_scale) = (0usize, 0usize, 1.0, 1.0);
        if unsafe { (api.SUTextureGetDimensions)(tex, &mut w, &mut h, &mut s_scale, &mut t_scale) } != SU_ERROR_NONE || w == 0 || h == 0 {
            return None;
        }
        let name = api.string(|s| unsafe { (api.SUTextureGetFileName)(tex, s) });
        // Keep the original extension: SketchUp writes the image back in its stored format, so a
        // JPEG texture must not be handed a ".png" path.
        let ext = Path::new(&name).extension().and_then(|e| e.to_str()).filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("png").to_lowercase();
        let n = self.model.textures.len();
        let file = self.tex_stem.with_file_name(format!("{}-tex{n}.{ext}", self.tex_stem.file_name()?.to_string_lossy()));
        if unsafe { (api.SUTextureWriteToFile)(tex, cstr(&file.to_string_lossy()).as_ptr()) } != SU_ERROR_NONE {
            return None;
        }
        // s/t scale are tiles per inch; invert for the model-space size of one tile.
        let size = |scale: f64| if scale.abs() > 1e-12 { 1.0 / scale.abs() } else { 1.0 };
        self.model.textures.push(SkpTexture {
            name,
            width: w as u32,
            height: h as u32,
            size_inches: [size(s_scale), size(t_scale)],
            file: file.file_name()?.to_string_lossy().into_owned(),
            data: Vec::new(),
        });
        Some(n)
    }

    /// How the face's texture lies in space, as an affine map (see [`SkpUv`]).
    ///
    /// SketchUp gives texture coordinates per point through a UV helper; three points that span
    /// the face determine the whole mapping, so we sample a well-spread triangle and solve for it.
    fn face_uv(&mut self, face: Ref, outer: &[[f64; 3]], normal: [f64; 3], front: bool, back: bool) -> (Option<SkpUv>, Option<SkpUv>) {
        let api = self.api;
        let Some([a, b, c]) = spread_triangle(outer) else { return (None, None) };
        let mut helper = Ref::NULL;
        if unsafe { (api.SUFaceGetUVHelper)(face, front, back, Ref::NULL, &mut helper) } != SU_ERROR_NONE || helper.is_null() {
            return (None, None);
        }
        let distorted = std::cell::Cell::new(false);
        let sample = |get: UvQ, p: [f64; 3]| -> Option<[f64; 2]> {
            let (pt, mut uvq) = (Pt::from(p), Pt::default());
            if unsafe { get(helper, &pt, &mut uvq) } != SU_ERROR_NONE || uvq.z.abs() < 1e-12 {
                return None;
            }
            // q is 1 for an ordinary texture; anything else means it is projected in perspective,
            // which an affine map can only approximate.
            if (uvq.z - 1.0).abs() > 1e-6 {
                distorted.set(true);
            }
            Some([uvq.x / uvq.z, uvq.y / uvq.z])
        };
        let solve_side = |get: UvQ| -> Option<SkpUv> { affine_uv([a, b, c], [sample(get, a)?, sample(get, b)?, sample(get, c)?], normal) };
        let f = if front { solve_side(api.SUUVHelperGetFrontUVQ) } else { None };
        let bk = if back { solve_side(api.SUUVHelperGetBackUVQ) } else { None };
        unsafe { (api.SUUVHelperRelease)(&mut helper) };
        if distorted.get() {
            self.distorted += 1;
        }
        (f, bk)
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
                let outer_pts = self.loop_points(outer);
                let holes: Vec<Vec<[f64; 3]>> = inner.into_iter().map(|l| self.loop_points(l)).collect();
                let (front, back) = (self.material(fm), self.material(bm));
                let textured = |m: Option<usize>| m.is_some_and(|i| self.model.materials[i].texture.is_some());
                let (ft, bt) = (textured(front), textured(back));
                let (front_uv, back_uv) = if ft || bt { self.face_uv(f, &outer_pts, normal.arr(), ft, bt) } else { (None, None) };
                out.faces.push(SkpFace { outer: outer_pts, holes, normal: normal.arr(), front, back, front_uv, back_uv });
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
