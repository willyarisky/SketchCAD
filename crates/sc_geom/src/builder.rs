//! Building a mesh from geometry that is already resolved (file import): vertices are welded,
//! but nothing is split and no faces are traced — the source's topology is kept as is.

use super::*;
use sc_math::DVec2;
use sc_math::geom2d::signed_area;

#[derive(Default)]
pub struct MeshBuilder {
    mesh: Mesh,
    grid: HashMap<(i64, i64, i64), Vec<VertexId>>,
    curves: HashMap<u64, u32>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn cell(p: DVec3) -> (i64, i64, i64) {
        let q = |v: f64| (v / (TOL * 4.0)).floor() as i64;
        (q(p.x), q(p.y), q(p.z))
    }

    /// The vertex at `p`, reusing one within tolerance.
    pub fn vertex(&mut self, p: DVec3) -> VertexId {
        let (cx, cy, cz) = Self::cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.grid.get(&(cx + dx, cy + dy, cz + dz))
                        && let Some(v) = list.iter().find(|v| points_eq(self.mesh.vertices[**v].pos, p)) {
                            return *v;
                        }
                }
            }
        }
        let v = self.mesh.insert_vertex_raw(p);
        self.grid.entry((cx, cy, cz)).or_default().push(v);
        v
    }

    /// An edge with its display flags; `curve` groups edges that came from one arc or circle.
    pub fn edge(&mut self, a: DVec3, b: DVec3, soft: bool, smooth: bool, hidden: bool, curve: Option<u64>) -> Option<EdgeId> {
        let (va, vb) = (self.vertex(a), self.vertex(b));
        if va == vb {
            return None;
        }
        let curve = curve.map(|c| {
            let next = self.curves.len() as u32 + 1;
            *self.curves.entry(c).or_insert(next)
        });
        let e = self.mesh.insert_edge_raw(va, vb, curve);
        let ed = &mut self.mesh.edges[e];
        (ed.soft, ed.smooth, ed.hidden) = (soft, smooth, hidden);
        if curve.is_some() {
            ed.curve = curve;
        }
        Some(e)
    }

    /// A face facing `normal`. Loop edges that don't exist yet are created (plain edges).
    /// A face already added, so callers can set properties `face` does not take (textures).
    pub fn face_mut(&mut self, f: FaceId) -> &mut Face {
        &mut self.mesh.faces[f]
    }

    pub fn face(&mut self, outer: &[DVec3], holes: &[Vec<DVec3>], normal: DVec3, front: Option<Color>, back: Option<Color>) -> Option<FaceId> {
        let n = normal.try_normalize()?;
        let mut lp = |pts: &[DVec3]| -> Vec<VertexId> {
            let mut out: Vec<VertexId> = Vec::with_capacity(pts.len());
            for p in pts {
                let v = self.vertex(*p);
                if out.last() != Some(&v) {
                    out.push(v);
                }
            }
            while out.len() > 1 && out.first() == out.last() {
                out.pop();
            }
            out
        };
        let mut outer = lp(outer);
        if outer.len() < 3 {
            return None;
        }
        let mut holes: Vec<Vec<VertexId>> = holes.iter().map(|h| lp(h)).filter(|h| h.len() >= 3).collect();
        let plane = Plane::new(self.mesh.pos(outer[0]), n);
        let area = |l: &[VertexId]| -> f64 {
            let pts: Vec<DVec2> = l.iter().map(|v| plane.to_2d(self.mesh.pos(*v))).collect();
            signed_area(&pts)
        };
        // Outer loops run counter-clockwise around the normal, holes clockwise.
        if area(&outer) < 0.0 {
            outer.reverse();
        }
        for h in &mut holes {
            if area(h) > 0.0 {
                h.reverse();
            }
        }
        let face = Face { outer, holes, plane, front, back, front_tex: None, back_tex: None };
        for (a, b) in face.loop_pairs() {
            self.mesh.insert_edge_raw(a, b, None);
        }
        self.mesh.insert_face_raw(face)
    }

    pub fn finish(mut self) -> Mesh {
        self.mesh.next_curve = self.curves.len() as u32;
        self.mesh.revision += 1;
        self.mesh
    }
}
