//! Ray picking and face triangulation helpers.

use super::*;
use sc_math::geom2d::{point_in_polygon, triangulate};
use sc_math::{DVec2, Ray};

#[derive(Clone, Copy, Debug)]
pub struct FaceHit {
    pub face: FaceId,
    pub t: f64,
    pub point: DVec3,
}

impl Mesh {
    /// Is `p` (assumed on the face plane) inside the face, excluding holes?
    pub fn face_contains(&self, f: FaceId, p: DVec3) -> bool {
        let face = &self.faces[f];
        let q = face.plane.to_2d(p);
        let poly = |l: &Vec<VertexId>| -> Vec<DVec2> { l.iter().map(|v| face.plane.to_2d(self.pos(*v))).collect() };
        point_in_polygon(q, &poly(&face.outer)) && !face.holes.iter().any(|h| point_in_polygon(q, &poly(h)))
    }

    /// Nearest face hit by `ray`, ignoring hits closer than `t_min`.
    pub fn pick_face(&self, ray: &Ray, t_min: f64) -> Option<FaceHit> {
        let mut best: Option<FaceHit> = None;
        for (id, f) in &self.faces {
            let Some(t) = f.plane.intersect_ray(ray) else { continue };
            if t < t_min || best.is_some_and(|b| t >= b.t) {
                continue;
            }
            let p = ray.at(t);
            if self.face_contains(id, p) {
                best = Some(FaceHit { face: id, t, point: p });
            }
        }
        best
    }

    /// Triangles of a face as vertex ids.
    pub fn triangulate_face_ids(&self, f: FaceId) -> Vec<[VertexId; 3]> {
        let face = &self.faces[f];
        let ids: Vec<VertexId> = face.vertex_ids().collect();
        let outer: Vec<DVec2> = face.outer.iter().map(|v| face.plane.to_2d(self.pos(*v))).collect();
        let holes: Vec<Vec<DVec2>> = face.holes.iter().map(|h| h.iter().map(|v| face.plane.to_2d(self.pos(*v))).collect()).collect();
        triangulate(&outer, &holes).into_iter().map(|t| [ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]]).collect()
    }

    /// Per-corner shading normals: faces meeting at a vertex across *smooth* edges share an
    /// averaged normal (SketchUp's "smooth" edges); everything else is flat.
    pub fn smooth_normals(&self) -> HashMap<(FaceId, VertexId), DVec3> {
        let mut out = HashMap::new();
        let mut by_vertex: HashMap<VertexId, Vec<FaceId>> = HashMap::new();
        let mut smooth_at: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
        for (eid, e) in &self.edges {
            if e.smooth && e.faces.len() >= 2 {
                for v in e.v {
                    smooth_at.entry(v).or_default().push(eid);
                    let list = by_vertex.entry(v).or_default();
                    for f in &e.faces {
                        if !list.contains(f) {
                            list.push(*f);
                        }
                    }
                }
            }
        }
        for (v, faces) in by_vertex {
            // Union faces joined by smooth edges incident to v.
            let mut group: Vec<usize> = (0..faces.len()).collect();
            fn find(g: &mut Vec<usize>, i: usize) -> usize {
                if g[i] != i {
                    let r = find(g, g[i]);
                    g[i] = r;
                }
                g[i]
            }
            for &e in &smooth_at[&v] {
                let fs: Vec<usize> = self.edges[e].faces.iter().filter_map(|f| faces.iter().position(|x| x == f)).collect();
                for w in fs.windows(2) {
                    let (a, b) = (find(&mut group, w[0]), find(&mut group, w[1]));
                    group[a] = b;
                }
            }
            for i in 0..faces.len() {
                let gi = find(&mut group, i);
                let mut n = DVec3::ZERO;
                for (j, f) in faces.iter().enumerate() {
                    if find(&mut group, j) == gi {
                        n += self.faces[*f].plane.normal;
                    }
                }
                if n.length() > 1e-9 {
                    out.insert((faces[i], v), n.normalize());
                }
            }
        }
        out
    }

    /// Triangles of a face in model space (outer loop CCW around the face normal).
    pub fn triangulate_face(&self, f: FaceId) -> Vec<[DVec3; 3]> {
        let face = &self.faces[f];
        let pts3: Vec<DVec3> = face.vertex_ids().map(|v| self.pos(v)).collect();
        let outer: Vec<DVec2> = face.outer.iter().map(|v| face.plane.to_2d(self.pos(*v))).collect();
        let holes: Vec<Vec<DVec2>> = face.holes.iter().map(|h| h.iter().map(|v| face.plane.to_2d(self.pos(*v))).collect()).collect();
        triangulate(&outer, &holes)
            .into_iter()
            .map(|t| [pts3[t[0] as usize], pts3[t[1] as usize], pts3[t[2] as usize]])
            .collect()
    }
}
