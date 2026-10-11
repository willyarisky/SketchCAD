//! SketchCAD geometry kernel: SketchUp-style "sticky" edges and faces.
//!
//! The model is a set of vertices, edges and planar faces (with holes). Edges split each other where
//! they cross, faces are created automatically whenever new edges close a coplanar loop, and
//! erasing an edge between two coplanar faces heals them into one.

mod builder;
mod faces;
mod instances;
mod ops;
mod pick;

pub use builder::MeshBuilder;
pub use instances::{Instance, transform_normal};
pub use ops::PushPullMode;
pub use pick::FaceHit;

use sc_math::{DMat4, DVec3, Plane, TOL, closest_on_segment, points_eq, segment_intersection};
use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};
use std::collections::{HashMap, HashSet};

new_key_type! {
    pub struct VertexId;
    pub struct EdgeId;
    pub struct FaceId;
    pub struct InstanceId;
    /// A component (or group) definition; owned by the model, not by a mesh.
    pub struct DefId;
}

/// sRGB color with alpha.
pub type Color = [u8; 4];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Vertex {
    pub pos: DVec3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub v: [VertexId; 2],
    #[serde(skip)]
    pub faces: Vec<FaceId>,
    pub soft: bool,
    pub smooth: bool,
    pub hidden: bool,
    /// Edges sharing a curve id (arcs, circles) select and edit as one entity.
    pub curve: Option<u32>,
}

impl Edge {
    pub fn other(&self, v: VertexId) -> VertexId {
        if self.v[0] == v { self.v[1] } else { self.v[0] }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Face {
    /// Outer loop, counter-clockwise around `plane.normal`.
    pub outer: Vec<VertexId>,
    /// Inner loops (holes), clockwise around `plane.normal`.
    pub holes: Vec<Vec<VertexId>>,
    /// Oriented plane: `plane.normal` is the front side.
    pub plane: Plane,
    pub front: Option<Color>,
    pub back: Option<Color>,
}

impl Face {
    pub fn loops(&self) -> impl Iterator<Item = &Vec<VertexId>> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }

    pub fn vertex_ids(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.loops().flat_map(|l| l.iter().copied())
    }

    /// Consecutive vertex pairs of every loop.
    pub fn loop_pairs(&self) -> Vec<(VertexId, VertexId)> {
        let mut out = Vec::new();
        for l in self.loops() {
            for i in 0..l.len() {
                out.push((l[i], l[(i + 1) % l.len()]));
            }
        }
        out
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mesh {
    pub vertices: SlotMap<VertexId, Vertex>,
    pub edges: SlotMap<EdgeId, Edge>,
    pub faces: SlotMap<FaceId, Face>,
    /// Group and component instances placed in this context.
    #[serde(default)]
    pub instances: SlotMap<InstanceId, Instance>,
    #[serde(skip)]
    edge_lookup: HashMap<(VertexId, VertexId), EdgeId>,
    next_curve: u32,
    /// Bumped on every change so renderers can cache derived data.
    #[serde(skip)]
    pub revision: u64,
}

#[inline]
fn key(a: VertexId, b: VertexId) -> (VertexId, VertexId) {
    if a < b { (a, b) } else { (b, a) }
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild derived lookup tables (after deserialization).
    pub fn rebuild_links(&mut self) {
        self.edge_lookup.clear();
        for (id, e) in &mut self.edges {
            e.faces.clear();
            self.edge_lookup.insert(key(e.v[0], e.v[1]), id);
        }
        let face_ids: Vec<FaceId> = self.faces.keys().collect();
        for f in face_ids {
            self.link_face(f);
        }
        self.revision += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.faces.is_empty() && self.instances.is_empty()
    }

    pub fn pos(&self, v: VertexId) -> DVec3 {
        self.vertices[v].pos
    }

    pub fn edge_points(&self, e: EdgeId) -> (DVec3, DVec3) {
        let e = &self.edges[e];
        (self.pos(e.v[0]), self.pos(e.v[1]))
    }

    pub fn edge_between(&self, a: VertexId, b: VertexId) -> Option<EdgeId> {
        self.edge_lookup.get(&key(a, b)).copied()
    }

    pub fn vertex_edges(&self, v: VertexId) -> Vec<EdgeId> {
        // Linear scan keeps the data model simple; an adjacency index comes with the BVH work.
        self.edges.iter().filter(|(_, e)| e.v[0] == v || e.v[1] == v).map(|(id, _)| id).collect()
    }

    pub fn new_curve_id(&mut self) -> u32 {
        self.next_curve += 1;
        self.next_curve
    }

    pub fn curve_edges(&self, curve: u32) -> Vec<EdgeId> {
        self.edges.iter().filter(|(_, e)| e.curve == Some(curve)).map(|(id, _)| id).collect()
    }

    pub fn find_vertex(&self, p: DVec3) -> Option<VertexId> {
        self.vertices.iter().find(|(_, v)| points_eq(v.pos, p)).map(|(id, _)| id)
    }

    pub fn bounds(&self) -> sc_math::Aabb {
        let mut b = sc_math::Aabb::EMPTY;
        for v in self.vertices.values() {
            b.extend(v.pos);
        }
        b
    }

    pub fn face_points(&self, f: FaceId) -> Vec<DVec3> {
        self.faces[f].outer.iter().map(|&v| self.pos(v)).collect()
    }

    /// Area of a face in square inches.
    pub fn face_area(&self, f: FaceId) -> f64 {
        let face = &self.faces[f];
        let poly = |l: &Vec<VertexId>| -> Vec<sc_math::DVec2> { l.iter().map(|&v| face.plane.to_2d(self.pos(v))).collect() };
        let mut a = sc_math::geom2d::signed_area(&poly(&face.outer)).abs();
        for h in &face.holes {
            a -= sc_math::geom2d::signed_area(&poly(h)).abs();
        }
        a
    }

    pub fn edge_length(&self, e: EdgeId) -> f64 {
        let (a, b) = self.edge_points(e);
        a.distance(b)
    }

    // -----------------------------------------------------------------------
    // Low-level construction

    fn insert_vertex_raw(&mut self, p: DVec3) -> VertexId {
        self.vertices.insert(Vertex { pos: p })
    }

    /// Insert an edge without any splitting. Returns the existing edge if one already joins `a` and `b`.
    fn insert_edge_raw(&mut self, a: VertexId, b: VertexId, curve: Option<u32>) -> EdgeId {
        if let Some(e) = self.edge_between(a, b) {
            return e;
        }
        let id = self.edges.insert(Edge { v: [a, b], faces: Vec::new(), soft: false, smooth: false, hidden: false, curve });
        self.edge_lookup.insert(key(a, b), id);
        id
    }

    fn link_face(&mut self, f: FaceId) {
        for (a, b) in self.faces[f].loop_pairs() {
            if let Some(e) = self.edge_between(a, b) {
                let faces = &mut self.edges[e].faces;
                if !faces.contains(&f) {
                    faces.push(f);
                }
            }
        }
    }

    fn insert_face_raw(&mut self, face: Face) -> Option<FaceId> {
        // Every loop edge must exist.
        for (a, b) in face.loop_pairs() {
            self.edge_between(a, b)?;
        }
        let id = self.faces.insert(face);
        self.link_face(id);
        Some(id)
    }

    fn remove_face_raw(&mut self, f: FaceId) -> Option<Face> {
        let face = self.faces.remove(f)?;
        for (a, b) in face.loop_pairs() {
            if let Some(e) = self.edge_between(a, b) {
                self.edges[e].faces.retain(|x| *x != f);
            }
        }
        Some(face)
    }

    fn remove_edge_raw(&mut self, e: EdgeId) -> Option<Edge> {
        let edge = self.edges.remove(e)?;
        self.edge_lookup.remove(&key(edge.v[0], edge.v[1]));
        Some(edge)
    }

    /// Split edge `e` at point `p` (assumed to lie on it). Faces using the edge get the new vertex inserted.
    pub fn split_edge(&mut self, e: EdgeId, p: DVec3) -> VertexId {
        let edge = self.edges[e].clone();
        let [a, b] = edge.v;
        if points_eq(self.pos(a), p) {
            return a;
        }
        if points_eq(self.pos(b), p) {
            return b;
        }
        let v = self.insert_vertex_raw(p);
        self.remove_edge_raw(e);
        let e1 = self.insert_edge_raw(a, v, edge.curve);
        let e2 = self.insert_edge_raw(v, b, edge.curve);
        for id in [e1, e2] {
            let ed = &mut self.edges[id];
            ed.soft = edge.soft;
            ed.smooth = edge.smooth;
            ed.hidden = edge.hidden;
        }
        for f in edge.faces {
            let face = &mut self.faces[f];
            for l in std::iter::once(&mut face.outer).chain(face.holes.iter_mut()) {
                let n = l.len();
                for i in 0..n {
                    let (x, y) = (l[i], l[(i + 1) % n]);
                    if (x == a && y == b) || (x == b && y == a) {
                        l.insert(i + 1, v);
                        break;
                    }
                }
            }
            for id in [e1, e2] {
                self.edges[id].faces.push(f);
            }
        }
        self.revision += 1;
        v
    }

    /// Existing vertex at `p`, or a vertex created by splitting an edge that passes through `p`,
    /// or a brand-new free vertex.
    fn vertex_at(&mut self, p: DVec3) -> VertexId {
        if let Some(v) = self.find_vertex(p) {
            return v;
        }
        let hit = self.edges.iter().find_map(|(id, e)| {
            let (a, b) = (self.pos(e.v[0]), self.pos(e.v[1]));
            let (t, d) = closest_on_segment(p, a, b);
            (d <= TOL && t > 0.0 && t < 1.0).then_some(id)
        });
        match hit {
            Some(e) => self.split_edge(e, p),
            None => self.insert_vertex_raw(p),
        }
    }

    // -----------------------------------------------------------------------
    // High-level drawing operations

    /// Add a single line, splitting and auto-creating faces like SketchUp's Line tool.
    pub fn add_line(&mut self, p0: DVec3, p1: DVec3) -> Vec<EdgeId> {
        self.add_segments(&[(p0, p1)], None)
    }

    /// Add a closed or open polyline. Use `curve = true` for circles/arcs so the edges select as one.
    pub fn add_polyline(&mut self, pts: &[DVec3], closed: bool, curve: bool) -> Vec<EdgeId> {
        let curve_id = curve.then(|| self.new_curve_id());
        let mut segs = Vec::new();
        for w in pts.windows(2) {
            segs.push((w[0], w[1]));
        }
        if closed && pts.len() > 2 {
            segs.push((pts[pts.len() - 1], pts[0]));
        }
        self.add_segments(&segs, curve_id)
    }

    /// Core of all drawing: insert segments with full splitting, then resolve faces once.
    pub fn add_segments(&mut self, segs: &[(DVec3, DVec3)], curve: Option<u32>) -> Vec<EdgeId> {
        let mut created = Vec::new();
        for &(p0, p1) in segs {
            if points_eq(p0, p1) {
                continue;
            }
            let va = self.vertex_at(p0);
            let vb = self.vertex_at(p1);
            let (pa, pb) = (self.pos(va), self.pos(vb));

            // Points along the new segment where it must be split: (param, vertex).
            let mut cuts: Vec<(f64, VertexId)> = vec![(0.0, va), (1.0, vb)];
            let len = pa.distance(pb);
            // Existing vertices lying on the segment interior.
            for (vid, v) in &self.vertices {
                if vid == va || vid == vb {
                    continue;
                }
                let (t, d) = closest_on_segment(v.pos, pa, pb);
                if d <= TOL && t * len > TOL && (1.0 - t) * len > TOL {
                    cuts.push((t, vid));
                }
            }
            // Crossings with existing edges (interior/interior).
            let crossings: Vec<(EdgeId, f64, DVec3)> = self
                .edges
                .iter()
                .filter_map(|(eid, e)| {
                    if e.v.contains(&va) || e.v.contains(&vb) {
                        return None;
                    }
                    let (a, b) = (self.pos(e.v[0]), self.pos(e.v[1]));
                    let (s, t) = segment_intersection(pa, pb, a, b)?;
                    let elen = a.distance(b);
                    let interior_new = s * len > TOL && (1.0 - s) * len > TOL;
                    let interior_old = t * elen > TOL && (1.0 - t) * elen > TOL;
                    (interior_new && interior_old).then(|| (eid, s, pa + (pb - pa) * s))
                })
                .collect();
            for (eid, s, p) in crossings {
                if self.edges.contains_key(eid) {
                    let v = self.split_edge(eid, p);
                    cuts.push((s, v));
                }
            }
            cuts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            cuts.dedup_by(|a, b| a.1 == b.1);
            for w in cuts.windows(2) {
                if w[0].1 != w[1].1 {
                    let existed = self.edge_between(w[0].1, w[1].1).is_some();
                    let e = self.insert_edge_raw(w[0].1, w[1].1, curve);
                    if !existed {
                        created.push(e);
                    }
                }
            }
        }
        self.resolve_faces_for(&created);
        self.revision += 1;
        created
    }

    /// Erase edges (and with them the faces they bound). Coplanar neighbours heal into one face.
    pub fn erase_edges(&mut self, edges: &[EdgeId]) {
        let edges: Vec<EdgeId> = edges.iter().copied().filter(|e| self.edges.contains_key(*e)).collect();
        if edges.is_empty() {
            return;
        }
        let mut cover: Vec<Face> = Vec::new();
        let mut doomed_faces = HashSet::new();
        let mut touched_vertices = HashSet::new();
        for &e in &edges {
            let ed = &self.edges[e];
            touched_vertices.extend(ed.v);
            if ed.faces.len() == 2 {
                let (f0, f1) = (&self.faces[ed.faces[0]], &self.faces[ed.faces[1]]);
                if f0.plane.coincident(&f1.plane) {
                    cover.push(f0.clone());
                    cover.push(f1.clone());
                }
            }
            doomed_faces.extend(ed.faces.iter().copied());
        }
        for f in doomed_faces {
            self.remove_face_raw(f);
        }
        for e in &edges {
            self.remove_edge_raw(*e);
        }
        // Heal coplanar faces.
        let mut planes: Vec<Plane> = Vec::new();
        for f in &cover {
            if !planes.iter().any(|p| p.coincident(&f.plane)) {
                planes.push(f.plane);
            }
        }
        for pl in planes {
            let cov: Vec<&Face> = cover.iter().filter(|f| f.plane.coincident(&pl)).collect();
            let cov: Vec<Face> = cov.into_iter().cloned().collect();
            self.recompute_plane(pl, &HashSet::new(), &cov);
        }
        for v in touched_vertices {
            self.heal_vertex(v);
        }
        self.revision += 1;
    }

    /// Erase faces only (edges stay).
    pub fn erase_faces(&mut self, faces: &[FaceId]) {
        for &f in faces {
            self.remove_face_raw(f);
        }
        self.revision += 1;
    }

    /// Remove a vertex if it has no edges, or merge its two edges if they are collinear.
    fn heal_vertex(&mut self, v: VertexId) {
        if !self.vertices.contains_key(v) {
            return;
        }
        let es = self.vertex_edges(v);
        if es.is_empty() {
            // Only drop it if no face references it.
            if !self.faces.values().any(|f| f.vertex_ids().any(|x| x == v)) {
                self.vertices.remove(v);
            }
            return;
        }
        if es.len() != 2 {
            return;
        }
        let (e0, e1) = (self.edges[es[0]].clone(), self.edges[es[1]].clone());
        let (a, b) = (e0.other(v), e1.other(v));
        let (pa, pv, pb) = (self.pos(a), self.pos(v), self.pos(b));
        if !sc_math::parallel(pv - pa, pb - pv) || (pv - pa).dot(pb - pv) <= 0.0 {
            return;
        }
        let mut f0 = e0.faces.clone();
        let mut f1 = e1.faces.clone();
        f0.sort();
        f1.sort();
        if f0 != f1 || self.edge_between(a, b).is_some() {
            return;
        }
        self.remove_edge_raw(es[0]);
        self.remove_edge_raw(es[1]);
        let curve = if e0.curve == e1.curve { e0.curve } else { None };
        let ne = self.insert_edge_raw(a, b, curve);
        self.edges[ne].soft = e0.soft && e1.soft;
        self.edges[ne].smooth = e0.smooth && e1.smooth;
        for f in &f0 {
            let face = &mut self.faces[*f];
            for l in std::iter::once(&mut face.outer).chain(face.holes.iter_mut()) {
                l.retain(|x| *x != v);
            }
            self.edges[ne].faces.push(*f);
        }
        self.vertices.remove(v);
    }

    /// Reverse a face's orientation (front ↔ back).
    pub fn reverse_face(&mut self, f: FaceId) {
        let face = &mut self.faces[f];
        face.outer.reverse();
        for h in &mut face.holes {
            h.reverse();
        }
        face.plane = face.plane.flipped();
        std::mem::swap(&mut face.front, &mut face.back);
        self.revision += 1;
    }

    /// Reverse a face's winding (and normal) without swapping its materials; used after mirroring.
    pub fn reverse_loops(&mut self, f: FaceId) {
        let face = &mut self.faces[f];
        face.outer.reverse();
        for h in &mut face.holes {
            h.reverse();
        }
        face.plane = face.plane.flipped();
        self.revision += 1;
    }

    /// Translate vertices. Faces touching moved vertices get their planes refit.
    pub fn move_vertices(&mut self, verts: &HashSet<VertexId>, delta: DVec3) {
        self.transform_vertices(verts, |p| p + delta);
    }

    pub fn transform_vertices(&mut self, verts: &HashSet<VertexId>, f: impl Fn(DVec3) -> DVec3) {
        for &v in verts {
            if let Some(vx) = self.vertices.get_mut(v) {
                vx.pos = f(vx.pos);
            }
        }
        let ids: Vec<FaceId> = self.faces.keys().collect();
        for fid in ids {
            if self.faces[fid].vertex_ids().any(|v| verts.contains(&v)) {
                self.refit_face_plane(fid);
            }
        }
        self.revision += 1;
    }

    fn refit_face_plane(&mut self, f: FaceId) {
        let pts = self.face_points(f);
        if let Some(pl) = Plane::from_polygon(&pts) {
            self.faces[f].plane = pl;
        }
    }

    /// Segments making up a set of edges plus the loops of a set of faces (deduplicated).
    pub fn source_segments(&self, edges: &[EdgeId], faces: &[FaceId]) -> Vec<(DVec3, DVec3)> {
        let mut all: HashSet<EdgeId> = edges.iter().copied().filter(|e| self.edges.contains_key(*e)).collect();
        for &f in faces {
            if let Some(face) = self.faces.get(f) {
                for (a, b) in face.loop_pairs() {
                    if let Some(e) = self.edge_between(a, b) {
                        all.insert(e);
                    }
                }
            }
        }
        all.into_iter().map(|e| self.edge_points(e)).collect()
    }

    /// Add copies of `segs` transformed by `xf`, merging them into existing geometry.
    /// Returns the newly created edges and the faces bounded by them.
    pub fn add_transformed(&mut self, segs: &[(DVec3, DVec3)], xf: impl Fn(DVec3) -> DVec3) -> (Vec<EdgeId>, Vec<FaceId>) {
        let segs: Vec<(DVec3, DVec3)> = segs.iter().map(|(a, b)| (xf(*a), xf(*b))).collect();
        let new_edges = self.add_segments(&segs, None);
        let new_set: HashSet<EdgeId> = new_edges.iter().copied().collect();
        let new_faces: Vec<FaceId> = self
            .faces
            .iter()
            .filter(|(_, f)| f.loop_pairs().iter().any(|(a, b)| self.edge_between(*a, *b).is_some_and(|e| new_set.contains(&e))))
            .map(|(id, _)| id)
            .collect();
        (new_edges, new_faces)
    }

    /// Copy edges and faces through `xf`, merging the copy into existing geometry.
    pub fn copy_entities(&mut self, edges: &[EdgeId], faces: &[FaceId], xf: impl Fn(DVec3) -> DVec3) -> (Vec<EdgeId>, Vec<FaceId>) {
        let segs = self.source_segments(edges, faces);
        self.add_transformed(&segs, xf)
    }

    /// Basic validity check used by tests and the future "Fix Problems" dialog.
    pub fn validate(&self) -> Result<(), String> {
        for (fid, f) in &self.faces {
            for (a, b) in f.loop_pairs() {
                let e = self.edge_between(a, b).ok_or_else(|| format!("face {fid:?} loop edge missing"))?;
                if !self.edges[e].faces.contains(&fid) {
                    return Err(format!("edge {e:?} missing face link {fid:?}"));
                }
            }
            for v in f.vertex_ids() {
                if !f.plane.contains(self.pos(v)) {
                    return Err(format!("face {fid:?} non-planar"));
                }
            }
        }
        for (eid, e) in &self.edges {
            if self.edge_between(e.v[0], e.v[1]) != Some(eid) {
                return Err(format!("edge lookup stale for {eid:?}"));
            }
            for f in &e.faces {
                if !self.faces.contains_key(*f) {
                    return Err(format!("edge {eid:?} links dead face"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
