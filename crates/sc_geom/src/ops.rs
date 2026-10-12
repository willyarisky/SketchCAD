//! Modeling operations built on top of the sticky-geometry primitives.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushPullMode {
    /// SketchUp default: stretch the solid when possible, otherwise extrude.
    Auto,
    /// Ctrl modifier: always create a new starting face and extrude from it.
    NewStartingFace,
}

impl Mesh {
    /// Push/Pull `face` by `distance` along its normal. Returns the face at the moved position.
    pub fn push_pull(&mut self, face: FaceId, distance: f64, mode: PushPullMode) -> Option<FaceId> {
        if distance.abs() <= TOL || !self.faces.contains_key(face) {
            return Some(face);
        }
        let f = self.faces[face].clone();
        let offset = f.plane.normal * distance;

        let neighbours: Vec<Option<FaceId>> = f
            .loop_pairs()
            .iter()
            .map(|(a, b)| {
                let e = self.edge_between(*a, *b)?;
                self.edges[e].faces.iter().copied().find(|x| *x != face)
            })
            .collect();
        let is_free = neighbours.iter().all(|n| n.is_none());
        let all_perpendicular = neighbours
            .iter()
            .all(|n| n.is_some_and(|nf| sc_math::perpendicular(self.faces[nf].plane.normal, f.plane.normal)));

        if mode == PushPullMode::Auto && !is_free && all_perpendicular {
            // Stretch: slide the face; the perpendicular neighbours stay planar and simply grow/shrink.
            let verts: HashSet<VertexId> = f.vertex_ids().collect();
            self.move_vertices(&verts, offset);
            return Some(face);
        }
        self.extrude(face, &f, offset, is_free || mode == PushPullMode::NewStartingFace)
    }

    fn extrude(&mut self, face: FaceId, f: &Face, offset: DVec3, keep_base: bool) -> Option<FaceId> {
        let forward = offset.dot(f.plane.normal) > 0.0;
        // Pushing an attached face backwards carves a hole: the swept region is empty space, so the
        // sides face into it and the moved face keeps its orientation. Otherwise the swept region is
        // new material and everything faces away from it.
        let carving = !keep_base && !forward;
        let mut map: HashMap<VertexId, VertexId> = HashMap::new();
        for v in f.vertex_ids() {
            let p = self.pos(v) + offset;
            let nv = self.find_vertex(p).unwrap_or_else(|| self.insert_vertex_raw(p));
            map.insert(v, nv);
        }
        // Edges: caps and sides.
        let mut base_edges = Vec::new();
        for (a, b) in f.loop_pairs() {
            let (ta, tb) = (map[&a], map[&b]);
            self.insert_edge_raw(ta, tb, None);
            self.insert_edge_raw(a, ta, None);
            if let Some(e) = self.edge_between(a, b) {
                base_edges.push(e);
            }
        }
        // Copy curve membership to the top loop so circles stay circles.
        for (a, b) in f.loop_pairs() {
            if let Some(e) = self.edge_between(a, b) {
                let c = self.edges[e].curve;
                if let Some(te) = self.edge_between(map[&a], map[&b]) {
                    self.edges[te].curve = c;
                }
            }
        }

        // The moved face keeps its orientation when pulling forward or carving.
        let mut top = Face {
            outer: f.outer.iter().map(|v| map[v]).collect(),
            holes: f.holes.iter().map(|h| h.iter().map(|v| map[v]).collect()).collect(),
            plane: Plane::new(self.pos(map[&f.outer[0]]), f.plane.normal),
            front: f.front,
            back: f.back,
            front_tex: f.front_tex,
            back_tex: f.back_tex,
        };
        if !forward && !carving {
            top.outer.reverse();
            for h in &mut top.holes {
                h.reverse();
            }
            top.plane = top.plane.flipped();
        }

        // Side quads, oriented away from the swept region.
        let mut sides = Vec::new();
        for (a, b) in f.loop_pairs() {
            let (ta, tb) = (map[&a], map[&b]);
            let quad = if forward || carving { vec![a, b, tb, ta] } else { vec![b, a, ta, tb] };
            let pts: Vec<DVec3> = quad.iter().map(|v| self.pos(*v)).collect();
            if let Some(pl) = Plane::from_polygon(&pts) {
                sides.push(Face { outer: quad, holes: Vec::new(), plane: pl, front: None, back: None, front_tex: None, back_tex: None });
            }
        }

        if keep_base {
            // The original face becomes the cap facing away from the extrusion.
            if forward {
                self.reverse_face(face);
            }
        } else {
            self.remove_face_raw(face);
        }
        let top_id = self.insert_face_raw(top);
        for s in sides {
            self.insert_face_raw(s);
        }

        // Heal seams where a side face continues a coplanar neighbour (stretching a wall).
        let seams: Vec<EdgeId> = base_edges
            .into_iter()
            .filter(|e| {
                let Some(ed) = self.edges.get(*e) else { return false };
                ed.faces.len() == 2 && {
                    let (f0, f1) = (&self.faces[ed.faces[0]], &self.faces[ed.faces[1]]);
                    f0.plane.normal.dot(f1.plane.normal) > 1.0 - sc_math::ANGLE_TOL && f0.plane.coincident(&f1.plane)
                }
            })
            .collect();
        if !seams.is_empty() {
            self.erase_edges(&seams);
        }
        self.revision += 1;
        top_id
    }
}

impl Mesh {
    /// Order a set of connected edges into a path (vertex chain). Returns the vertices and whether
    /// the path is closed. `None` if the edges don't form a single simple chain.
    pub fn edge_chain(&self, edges: &[EdgeId]) -> Option<(Vec<VertexId>, bool)> {
        let set: HashSet<EdgeId> = edges.iter().copied().filter(|e| self.edges.contains_key(*e)).collect();
        if set.is_empty() {
            return None;
        }
        let mut adj: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
        for &e in &set {
            let [a, b] = self.edges[e].v;
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        if adj.values().any(|n| n.len() > 2) {
            return None;
        }
        let start = adj.iter().find(|(_, n)| n.len() == 1).map(|(v, _)| *v);
        let closed = start.is_none();
        let start = start.unwrap_or_else(|| *adj.keys().next().unwrap());
        let mut chain = vec![start];
        let mut prev: Option<VertexId> = None;
        let mut cur = start;
        loop {
            let next = adj[&cur].iter().copied().find(|n| Some(*n) != prev && (chain.len() < 2 || *n != chain[chain.len() - 2]));
            let Some(n) = next else { break };
            if n == start {
                break;
            }
            chain.push(n);
            prev = Some(cur);
            cur = n;
        }
        (chain.len() == adj.len()).then_some((chain, closed))
    }

    /// Follow Me: sweep `face` along the path through `path` vertices (positions), mitering at
    /// corners. The profile is carried from the path start along the first segment, so it need not
    /// sit exactly on the path. Returns the new faces.
    pub fn follow_me(&mut self, face: FaceId, path: &[DVec3], closed: bool, path_soft: &[bool]) -> Vec<FaceId> {
        let Some(f) = self.faces.get(face).cloned() else { return Vec::new() };
        let n = path.len();
        if n < 2 {
            return Vec::new();
        }
        // Segment directions and miter-plane normals at each path vertex.
        let seg = |i: usize| (path[(i + 1) % n] - path[i]).normalize();
        let nseg = if closed { n } else { n - 1 };
        let miter = |i: usize| -> DVec3 {
            if !closed && i == 0 {
                return seg(0);
            }
            if !closed && i == n - 1 {
                return seg(n - 2);
            }
            let (din, dout) = (seg((i + n - 1) % n), seg(i % n));
            let b = din + dout;
            if b.length() < 1e-9 { dout } else { b.normalize() }
        };
        let ring_count = n;
        let loops: Vec<Vec<VertexId>> = f.loops().cloned().collect();
        let loop_pts: Vec<Vec<DVec3>> = loops.iter().map(|l| l.iter().map(|v| self.pos(*v)).collect()).collect();

        // Rings: profile points carried along the path from plane to plane.
        let project = |p: DVec3, d: DVec3, origin: DVec3, normal: DVec3| -> DVec3 {
            let denom = d.dot(normal);
            if denom.abs() < 1e-12 { p } else { p + d * ((origin - p).dot(normal) / denom) }
        };
        let mut rings: Vec<Vec<Vec<DVec3>>> = Vec::with_capacity(ring_count);
        let first: Vec<Vec<DVec3>> = loop_pts.iter().map(|l| l.iter().map(|p| project(*p, seg(0), path[0], miter(0))).collect()).collect();
        rings.push(first);
        for i in 1..ring_count {
            let d = seg(i - 1);
            let prev = rings[i - 1].clone();
            rings.push(prev.iter().map(|l| l.iter().map(|p| project(*p, d, path[i], miter(i))).collect()).collect());
        }

        // Vertices (reusing the profile's own vertices where the first ring coincides with them).
        let vid = |m: &mut Mesh, p: DVec3| m.find_vertex(p).unwrap_or_else(|| m.insert_vertex_raw(p));
        let ring_ids: Vec<Vec<Vec<VertexId>>> = rings.iter().map(|r| r.iter().map(|l| l.iter().map(|p| vid(self, *p)).collect()).collect()).collect();

        // Which profile vertices sit between two edges of the same curve (their sweep edges go soft).
        let soft_profile: Vec<Vec<bool>> = loops
            .iter()
            .map(|l| {
                (0..l.len())
                    .map(|j| {
                        let (p, c, nx) = (l[(j + l.len() - 1) % l.len()], l[j], l[(j + 1) % l.len()]);
                        let c1 = self.edge_between(p, c).and_then(|e| self.edges[e].curve);
                        let c2 = self.edge_between(c, nx).and_then(|e| self.edges[e].curve);
                        c1.is_some() && c1 == c2
                    })
                    .collect()
            })
            .collect();

        self.remove_face_raw(face);
        let mut new_faces = Vec::new();
        for s in 0..nseg {
            let (r0, r1) = (&ring_ids[s], &ring_ids[(s + 1) % ring_count]);
            for (li, l0) in r0.iter().enumerate() {
                let l1 = &r1[li];
                let m = l0.len();
                for j in 0..m {
                    let k = (j + 1) % m;
                    let quad = vec![l0[j], l0[k], l1[k], l1[j]];
                    let mut uniq = quad.clone();
                    uniq.dedup();
                    if uniq.len() < 3 || quad[0] == quad[3] && quad[1] == quad[2] {
                        continue;
                    }
                    for (a, b) in [(l0[j], l0[k]), (l0[k], l1[k]), (l1[k], l1[j]), (l1[j], l0[j])] {
                        if a != b {
                            self.insert_edge_raw(a, b, None);
                        }
                    }
                    let mut poly: Vec<VertexId> = quad;
                    poly.dedup();
                    if poly.first() == poly.last() {
                        poly.pop();
                    }
                    let pts: Vec<DVec3> = poly.iter().map(|v| self.pos(*v)).collect();
                    if let Some(pl) = Plane::from_polygon(&pts)
                        && let Some(id) = self.insert_face_raw(Face { outer: poly, holes: Vec::new(), plane: pl, front: f.front, back: f.back, front_tex: f.front_tex, back_tex: f.back_tex }) {
                            new_faces.push(id);
                        }
                    // Smooth the sweep along curved profiles and around curved path corners.
                    if soft_profile[li][j]
                        && let Some(e) = self.edge_between(l0[j], l1[j]) {
                            self.edges[e].soft = true;
                            self.edges[e].smooth = true;
                        }
                }
                let vi = (s + 1) % ring_count;
                if path_soft.get(vi).copied().unwrap_or(false) && (closed || vi != ring_count - 1) {
                    for j in 0..m {
                        if let Some(e) = self.edge_between(l1[j], l1[(j + 1) % m]) {
                            self.edges[e].soft = true;
                            self.edges[e].smooth = true;
                        }
                    }
                }
            }
        }
        // Caps for an open path.
        if !closed {
            for ring in [&ring_ids[0], &ring_ids[ring_count - 1]] {
                for l in ring {
                    for j in 0..l.len() {
                        self.insert_edge_raw(l[j], l[(j + 1) % l.len()], None);
                    }
                }
                let outer = ring[0].clone();
                let pts: Vec<DVec3> = outer.iter().map(|v| self.pos(*v)).collect();
                if let Some(pl) = Plane::from_polygon(&pts) {
                    let mut holes: Vec<Vec<VertexId>> = ring[1..].to_vec();
                    for h in &mut holes {
                        h.reverse();
                    }
                    if let Some(id) = self.insert_face_raw(Face { outer, holes, plane: pl, front: f.front, back: f.back, front_tex: f.front_tex, back_tex: f.back_tex }) {
                        new_faces.push(id);
                    }
                }
            }
        }
        // Orient everything outward: a closed sweep must have positive signed volume.
        let vol: f64 = new_faces
            .iter()
            .flat_map(|id| self.triangulate_face(*id))
            .map(|t| t[0].dot(t[1].cross(t[2])) / 6.0)
            .sum();
        if vol < 0.0 {
            for id in &new_faces {
                self.reverse_face(*id);
            }
        }
        self.revision += 1;
        new_faces
    }
}
