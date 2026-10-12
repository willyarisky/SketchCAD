//! Automatic face creation, splitting and merging.
//!
//! For every plane touched by an edit we trace all bounded regions formed by the edges lying in that
//! plane (classic planar-graph face tracing), then reconcile them with the faces that existed before:
//! regions covered by an old face inherit it (this is how a line across a face splits it), regions
//! bounded by a freshly drawn edge become new faces, and everything else stays open.

use super::*;
use sc_math::DVec2;
use sc_math::geom2d::{interior_point, point_in_polygon, signed_area};

const AREA_EPS: f64 = 1e-9;

struct Region {
    outer: Vec<VertexId>,
    holes: Vec<Vec<VertexId>>,
}

impl Mesh {
    /// Re-evaluate faces in every plane that the given new edges could close a loop in.
    pub(crate) fn resolve_faces_for(&mut self, new_edges: &[EdgeId]) {
        if new_edges.is_empty() {
            return;
        }
        let new_set: HashSet<EdgeId> = new_edges.iter().copied().collect();
        let mut planes: Vec<Plane> = Vec::new();
        let push_plane = |pl: Plane, planes: &mut Vec<Plane>| {
            if !planes.iter().any(|p| p.coincident(&pl)) {
                planes.push(pl);
            }
        };
        for &e in new_edges {
            let Some(ed) = self.edges.get(e) else { continue };
            let [a, b] = ed.v;
            let (pa, pb) = (self.pos(a), self.pos(b));
            for f in self.faces.values() {
                if f.plane.contains(pa) && f.plane.contains(pb) {
                    push_plane(f.plane, &mut planes);
                }
            }
            for v in [a, b] {
                for e2 in self.vertex_edges(v) {
                    if e2 == e {
                        continue;
                    }
                    let c = self.pos(self.edges[e2].other(v));
                    if let Some(pl) = Plane::from_points(pa, pb, c) {
                        push_plane(pl, &mut planes);
                    }
                }
            }
        }
        for pl in planes {
            self.recompute_plane(pl, &new_set, &[]);
        }
    }

    /// Rebuild the faces lying in `plane`.
    ///
    /// * `new_edges` — regions whose outer loop uses one of these become faces even if uncovered.
    /// * `extra_cover` — faces that no longer exist but whose area should still be filled (healing).
    pub(crate) fn recompute_plane(&mut self, plane: Plane, new_edges: &HashSet<EdgeId>, extra_cover: &[Face]) {
        // 1. Edges in the plane.
        let planar: Vec<EdgeId> = self
            .edges
            .iter()
            .filter(|(_, e)| plane.contains(self.pos(e.v[0])) && plane.contains(self.pos(e.v[1])))
            .map(|(id, _)| id)
            .collect();
        if planar.len() < 2 && extra_cover.is_empty() {
            return;
        }

        // 2. Adjacency sorted by angle around each vertex.
        let mut uv: HashMap<VertexId, DVec2> = HashMap::new();
        let mut adj: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
        for &e in &planar {
            let [a, b] = self.edges[e].v;
            for v in [a, b] {
                uv.entry(v).or_insert_with(|| plane.to_2d(self.pos(v)));
            }
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let angle = |from: VertexId, to: VertexId| -> f64 {
            let d = uv[&to] - uv[&from];
            d.y.atan2(d.x)
        };
        for (v, ns) in adj.iter_mut() {
            ns.sort_by(|x, y| angle(*v, *x).partial_cmp(&angle(*v, *y)).unwrap());
        }

        // Connected components (for hole assignment).
        let mut comp: HashMap<VertexId, usize> = HashMap::new();
        let mut ncomp = 0;
        for &start in adj.keys() {
            if comp.contains_key(&start) {
                continue;
            }
            let mut stack = vec![start];
            comp.insert(start, ncomp);
            while let Some(v) = stack.pop() {
                for &w in &adj[&v] {
                    if let std::collections::hash_map::Entry::Vacant(e) = comp.entry(w) {
                        e.insert(ncomp);
                        stack.push(w);
                    }
                }
            }
            ncomp += 1;
        }

        // 3. Trace cycles: after arriving at v from u, leave along the next neighbour clockwise from u.
        let mut visited: HashSet<(VertexId, VertexId)> = HashSet::new();
        let mut cycles: Vec<(Vec<VertexId>, f64)> = Vec::new();
        for (&u0, ns) in &adj {
            for &v0 in ns {
                if visited.contains(&(u0, v0)) {
                    continue;
                }
                let mut cyc = Vec::new();
                let (mut u, mut v) = (u0, v0);
                let mut guard = 0;
                loop {
                    visited.insert((u, v));
                    cyc.push(u);
                    let nv = &adj[&v];
                    let idx = nv.iter().position(|x| *x == u).unwrap();
                    let w = nv[(idx + nv.len() - 1) % nv.len()];
                    u = v;
                    v = w;
                    guard += 1;
                    if (u, v) == (u0, v0) || guard > 100_000 {
                        break;
                    }
                }
                let pts: Vec<DVec2> = cyc.iter().map(|x| uv[x]).collect();
                let area = signed_area(&pts);
                cycles.push((strip_spurs(cyc), area));
            }
        }

        // 4. Bounded regions (CCW) and component boundaries (CW) that become holes.
        let poly = |l: &[VertexId]| -> Vec<DVec2> { l.iter().map(|x| uv[x]).collect() };
        let mut regions: Vec<(Region, f64)> = Vec::new();
        let mut boundaries: Vec<Vec<VertexId>> = Vec::new();
        for (cyc, area) in cycles {
            if cyc.len() < 3 {
                continue;
            }
            if area > AREA_EPS {
                regions.push((Region { outer: cyc, holes: Vec::new() }, area));
            } else if area < -AREA_EPS {
                boundaries.push(cyc);
            }
        }
        for b in boundaries {
            let c = comp[&b[0]];
            let probe = uv[&b[0]];
            let best = regions
                .iter_mut()
                .filter(|(r, _)| comp[&r.outer[0]] != c && point_in_polygon(probe, &poly(&r.outer)))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            if let Some((r, _)) = best {
                r.holes.push(b);
            }
        }
        // A hole inside a hole's own nested region must belong only to the innermost container.
        // min_by(area) above already picks the innermost bounded region.

        // 5. Old faces in this plane.
        let old: Vec<FaceId> = self
            .faces
            .iter()
            .filter(|(_, f)| f.plane.coincident(&plane) && f.vertex_ids().all(|v| plane.contains(self.pos(v))))
            .map(|(id, _)| id)
            .collect();
        struct Cover {
            outer: Vec<DVec2>,
            holes: Vec<Vec<DVec2>>,
            normal: DVec3,
            front: Option<Color>,
            back: Option<Color>,
            front_tex: Option<FaceTexture>,
            back_tex: Option<FaceTexture>,
        }
        let to_cover = |f: &Face| Cover {
            outer: f.outer.iter().map(|v| plane.to_2d(self.pos(*v))).collect(),
            holes: f.holes.iter().map(|h| h.iter().map(|v| plane.to_2d(self.pos(*v))).collect()).collect(),
            normal: f.plane.normal,
            front: f.front,
            back: f.back,
            front_tex: f.front_tex,
            back_tex: f.back_tex,
        };
        let covers: Vec<(Option<FaceId>, Cover)> = old
            .iter()
            .map(|id| (Some(*id), to_cover(&self.faces[*id])))
            .chain(extra_cover.iter().map(|f| (None, to_cover(f))))
            .collect();

        // 6. Decide which regions are faces.
        let mut keep: HashSet<FaceId> = HashSet::new();
        let mut to_create: Vec<Face> = Vec::new();
        for (r, _) in regions {
            let outer2 = poly(&r.outer);
            let holes2: Vec<Vec<DVec2>> = r.holes.iter().map(|h| poly(h)).collect();
            let Some(sample) = interior_point(&outer2, &holes2) else { continue };
            let covered = covers.iter().find(|(_, c)| point_in_polygon(sample, &c.outer) && !c.holes.iter().any(|h| point_in_polygon(sample, h)));
            let uses_new = r.outer.iter().enumerate().any(|(i, &a)| {
                let b = r.outer[(i + 1) % r.outer.len()];
                self.edge_between(a, b).is_some_and(|e| new_edges.contains(&e))
            });
            let (normal, front, back, front_tex, back_tex) = match covered {
                Some((Some(fid), c)) => {
                    if same_region(&self.faces[*fid], &r) {
                        keep.insert(*fid);
                        continue;
                    }
                    (c.normal, c.front, c.back, c.front_tex, c.back_tex)
                }
                Some((None, c)) => (c.normal, c.front, c.back, c.front_tex, c.back_tex),
                None if uses_new => (self.default_normal(&plane, &r.outer), None, None, None, None),
                None => continue,
            };
            let mut face = Face { outer: r.outer, holes: r.holes, plane, front, back, front_tex, back_tex };
            // Traced loops are CCW around `plane.normal`; holes are CW.
            for h in &mut face.holes {
                if signed_area(&poly(h)) > 0.0 {
                    h.reverse();
                }
            }
            if normal.dot(plane.normal) < 0.0 {
                face.outer.reverse();
                for h in &mut face.holes {
                    h.reverse();
                }
                face.plane = plane.flipped();
            }
            to_create.push(face);
        }

        // 7. Replace changed faces.
        for id in old {
            if !keep.contains(&id) {
                self.remove_face_raw(id);
            }
        }
        for f in to_create {
            self.insert_face_raw(f);
        }
    }

    /// Orientation for a face that is not inheriting from an old one.
    ///
    /// Like SketchUp: faces on the ground plane face down (so the back side shows from above),
    /// otherwise orient consistently with an adjacent face when there is one.
    fn default_normal(&self, plane: &Plane, outer: &[VertexId]) -> DVec3 {
        let n = plane.normal;
        if n.z.abs() > 1.0 - 1e-9 && plane.contains(DVec3::ZERO) {
            return -DVec3::Z;
        }
        // Shared edge used in the same direction by a neighbour means we must flip to stay consistent.
        for i in 0..outer.len() {
            let (a, b) = (outer[i], outer[(i + 1) % outer.len()]);
            let Some(e) = self.edge_between(a, b) else { continue };
            for &f in &self.edges[e].faces {
                let face = &self.faces[f];
                if face.plane.coincident(plane) {
                    continue;
                }
                for (x, y) in face.loop_pairs() {
                    if x == a && y == b {
                        return -n;
                    }
                    if x == b && y == a {
                        return n;
                    }
                }
            }
        }
        if n.z.abs() > 1.0 - 1e-9 {
            return DVec3::Z;
        }
        n
    }
}

/// Remove back-and-forth spurs (`… x y x …`) left by dangling edges.
fn strip_spurs(mut cyc: Vec<VertexId>) -> Vec<VertexId> {
    loop {
        let n = cyc.len();
        if n < 3 {
            return cyc;
        }
        let mut removed = false;
        for i in 0..n {
            let prev = cyc[(i + n - 1) % n];
            let next = cyc[(i + 1) % n];
            if prev == next {
                // Drop `cyc[i]` and one copy of `prev`.
                let j = (i + 1) % n;
                let (hi, lo) = if i > j { (i, j) } else { (j, i) };
                cyc.remove(hi);
                cyc.remove(lo);
                removed = true;
                break;
            }
        }
        if !removed {
            return cyc;
        }
    }
}

fn same_region(f: &Face, r: &Region) -> bool {
    let canon = |l: &[VertexId]| {
        let mut v = l.to_vec();
        v.sort();
        v
    };
    if canon(&f.outer) != canon(&r.outer) || f.holes.len() != r.holes.len() {
        return false;
    }
    let mut a: Vec<Vec<VertexId>> = f.holes.iter().map(|h| canon(h)).collect();
    let mut b: Vec<Vec<VertexId>> = r.holes.iter().map(|h| canon(h)).collect();
    a.sort();
    b.sort();
    a == b
}
