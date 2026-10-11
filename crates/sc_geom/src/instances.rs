//! Group/component instances and the operations that move geometry in and out of them.

use super::*;
use sc_math::DVec2;
use sc_math::geom2d::interior_point;

/// A placed group or component. The geometry lives in the model's definition `def`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub def: DefId,
    /// Definition space → parent space.
    pub xform: DMat4,
    /// Instance name (Entity Info / Outliner); empty means "use the definition name".
    pub name: String,
    /// Index into the model's tag list; `None` is "Untagged".
    pub tag: Option<u32>,
    pub hidden: bool,
    pub locked: bool,
}

impl Instance {
    pub fn new(def: DefId, xform: DMat4) -> Self {
        Self { def, xform, name: String::new(), tag: None, hidden: false, locked: false }
    }
}

/// Transform a normal by an affine matrix (inverse-transpose), renormalized.
pub fn transform_normal(xf: &DMat4, n: DVec3) -> DVec3 {
    let m = sc_math::DMat3::from_mat4(*xf).inverse().transpose();
    (m * n).normalize()
}

impl Mesh {
    /// Move the given entities into a new mesh (for Make Group / Make Component).
    ///
    /// Edges shared with faces that stay behind are copied, not moved, so the remaining geometry
    /// keeps its boundaries — the same as SketchUp.
    pub fn extract(&mut self, edges: &[EdgeId], faces: &[FaceId], instances: &[InstanceId]) -> Mesh {
        let mut out = Mesh::new();
        out.next_curve = self.next_curve;
        let mut vmap: HashMap<VertexId, VertexId> = HashMap::new();
        let mut all_edges: Vec<EdgeId> = edges.iter().copied().filter(|e| self.edges.contains_key(*e)).collect();
        for &f in faces {
            if let Some(face) = self.faces.get(f) {
                for (a, b) in face.loop_pairs() {
                    if let Some(e) = self.edge_between(a, b) {
                        all_edges.push(e);
                    }
                }
            }
        }
        all_edges.sort();
        all_edges.dedup();
        let mut map_v = |out: &mut Mesh, v: VertexId, pos: DVec3| *vmap.entry(v).or_insert_with(|| out.insert_vertex_raw(pos));
        for &e in &all_edges {
            let ed = self.edges[e].clone();
            let a = map_v(&mut out, ed.v[0], self.pos(ed.v[0]));
            let b = map_v(&mut out, ed.v[1], self.pos(ed.v[1]));
            let ne = out.insert_edge_raw(a, b, ed.curve);
            let n = &mut out.edges[ne];
            (n.soft, n.smooth, n.hidden) = (ed.soft, ed.smooth, ed.hidden);
        }
        for &f in faces {
            let Some(face) = self.faces.get(f) else { continue };
            let mut nf = face.clone();
            nf.outer = nf.outer.iter().map(|v| map_v(&mut out, *v, self.pos(*v))).collect();
            nf.holes = nf.holes.iter().map(|h| h.iter().map(|v| map_v(&mut out, *v, self.pos(*v))).collect()).collect();
            out.insert_face_raw(nf);
        }
        for &i in instances {
            if let Some(inst) = self.instances.remove(i) {
                out.instances.insert(inst);
            }
        }

        // Remove from self.
        let mut touched = HashSet::new();
        for &f in faces {
            if let Some(face) = self.remove_face_raw(f) {
                touched.extend(face.vertex_ids());
            }
        }
        for &e in &all_edges {
            if self.edges.get(e).is_some_and(|ed| ed.faces.is_empty())
                && let Some(ed) = self.remove_edge_raw(e) {
                    touched.extend(ed.v);
                }
        }
        for v in touched {
            let used = self.edges.values().any(|e| e.v.contains(&v)) || self.faces.values().any(|f| f.vertex_ids().any(|x| x == v));
            if !used {
                self.vertices.remove(v);
            }
        }
        self.revision += 1;
        out.revision += 1;
        out
    }

    /// Merge `other` (transformed by `xf`) into this mesh, as Explode does.
    ///
    /// Edges are added with full splitting; faces keep their materials and orientation; loops that
    /// were not faces in `other` do not become faces. Returns the edges, faces and instances added.
    pub fn merge_mesh(&mut self, other: &Mesh, xf: DMat4) -> (Vec<EdgeId>, Vec<FaceId>, Vec<InstanceId>) {
        let t = |p: DVec3| xf.transform_point3(p);
        let faces_before: HashSet<FaceId> = self.faces.keys().collect();
        let segs: Vec<(DVec3, DVec3)> = other.edges.values().map(|e| (t(other.pos(e.v[0])), t(other.pos(e.v[1])))).collect();
        let new_edges = self.add_segments(&segs, None);

        // Edge attributes (soft/smooth/hidden) and curves.
        let mut curve_map: HashMap<u32, u32> = HashMap::new();
        for e in other.edges.values() {
            let (Some(a), Some(b)) = (self.find_vertex(t(other.pos(e.v[0]))), self.find_vertex(t(other.pos(e.v[1])))) else { continue };
            let Some(ne) = self.edge_between(a, b) else { continue };
            let curve = e.curve.map(|c| *curve_map.entry(c).or_insert_with(|| {
                self.next_curve += 1;
                self.next_curve
            }));
            let n = &mut self.edges[ne];
            (n.soft, n.smooth, n.hidden, n.curve) = (e.soft, e.smooth, e.hidden, curve);
        }

        // Faces: match each source face to the target face covering its interior.
        let mut matched = HashSet::new();
        for f in other.faces.values() {
            let outer: Vec<DVec2> = f.outer.iter().map(|v| f.plane.to_2d(other.pos(*v))).collect();
            let holes: Vec<Vec<DVec2>> = f.holes.iter().map(|h| h.iter().map(|v| f.plane.to_2d(other.pos(*v))).collect()).collect();
            let Some(ip) = interior_point(&outer, &holes) else { continue };
            let p = t(f.plane.to_3d(ip));
            let n = transform_normal(&xf, f.plane.normal);
            let target = self.faces.iter().find(|(_, tf)| tf.plane.contains(p) && sc_math::parallel(tf.plane.normal, n)).map(|(id, _)| id);
            let target = target.filter(|id| self.face_contains(*id, p));
            if let Some(id) = target {
                if self.faces[id].plane.normal.dot(n) < 0.0 {
                    self.reverse_face(id);
                }
                let tf = &mut self.faces[id];
                tf.front = f.front;
                tf.back = f.back;
                matched.insert(id);
            }
        }
        let unmatched_new: Vec<FaceId> = self.faces.keys().filter(|id| !faces_before.contains(id) && !matched.contains(id)).collect();
        for id in unmatched_new {
            self.remove_face_raw(id);
        }
        let new_faces: Vec<FaceId> = matched.into_iter().filter(|id| !faces_before.contains(id)).collect();

        let new_instances = other
            .instances
            .values()
            .map(|i| {
                let mut c = i.clone();
                c.xform = xf * i.xform;
                self.instances.insert(c)
            })
            .collect();
        self.revision += 1;
        (new_edges, new_faces, new_instances)
    }

    /// Append a transformed copy of `other`'s vertices, edges and faces without any merging.
    /// Used to build flattened copies for picking, inference and rendering.
    pub fn append_raw(&mut self, other: &Mesh, xf: &DMat4) {
        let mut vmap: HashMap<VertexId, VertexId> = HashMap::with_capacity(other.vertices.len());
        for (id, v) in &other.vertices {
            vmap.insert(id, self.vertices.insert(Vertex { pos: xf.transform_point3(v.pos) }));
        }
        for e in other.edges.values() {
            let ne = self.insert_edge_raw(vmap[&e.v[0]], vmap[&e.v[1]], None);
            let n = &mut self.edges[ne];
            (n.soft, n.smooth, n.hidden) = (e.soft, e.smooth, e.hidden);
        }
        for f in other.faces.values() {
            let p0 = xf.transform_point3(f.plane.origin());
            let n = transform_normal(xf, f.plane.normal);
            let mut face = Face {
                outer: f.outer.iter().map(|v| vmap[v]).collect(),
                holes: f.holes.iter().map(|h| h.iter().map(|v| vmap[v]).collect()).collect(),
                plane: Plane::new(p0, n),
                front: f.front,
                back: f.back,
            };
            // A mirroring transform flips loop winding; keep loops CCW around the normal.
            if xf.determinant() < 0.0 {
                face.outer.reverse();
                for h in &mut face.holes {
                    h.reverse();
                }
            }
            self.insert_face_raw(face);
        }
        self.revision += 1;
    }
}
