//! Core f64 math for SketchCAD.
//!
//! All model-space values are `f64` and expressed in **inches**, matching SketchUp's
//! internal unit. Conversion to display units happens only at the UI boundary (see [`units`]).

pub mod geom2d;
pub mod units;

pub use glam::{DMat3, DMat4, DQuat, DVec2, DVec3, DVec4};
use serde::{Deserialize, Serialize};

/// Length tolerance in inches (SketchUp merges vertices closer than ~0.001").
pub const TOL: f64 = 1.0e-3;
/// Tolerance used for unit-vector comparisons (dot products, cross magnitudes).
pub const ANGLE_TOL: f64 = 1.0e-6;

pub const RED_AXIS: DVec3 = DVec3::X;
pub const GREEN_AXIS: DVec3 = DVec3::Y;
pub const BLUE_AXIS: DVec3 = DVec3::Z;

#[inline]
pub fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOL
}

#[inline]
pub fn points_eq(a: DVec3, b: DVec3) -> bool {
    a.distance_squared(b) <= TOL * TOL
}

/// True when `a` and `b` are parallel (same or opposite direction). Inputs need not be normalized.
pub fn parallel(a: DVec3, b: DVec3) -> bool {
    let (la, lb) = (a.length(), b.length());
    if la < 1e-12 || lb < 1e-12 {
        return false;
    }
    a.cross(b).length() / (la * lb) < ANGLE_TOL * 100.0
}

/// True when `a` and `b` are perpendicular. Inputs need not be normalized.
pub fn perpendicular(a: DVec3, b: DVec3) -> bool {
    let (la, lb) = (a.length(), b.length());
    if la < 1e-12 || lb < 1e-12 {
        return false;
    }
    (a.dot(b) / (la * lb)).abs() < ANGLE_TOL * 100.0
}

/// Any unit vector perpendicular to `n`.
pub fn any_perpendicular(n: DVec3) -> DVec3 {
    let n = n.normalize();
    let helper = if n.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    n.cross(helper).normalize()
}

// ---------------------------------------------------------------------------

/// An infinite plane `normal · p = d` with a unit normal.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub normal: DVec3,
    pub d: f64,
}

impl Plane {
    pub fn new(point: DVec3, normal: DVec3) -> Self {
        let normal = normal.normalize();
        Self { normal, d: normal.dot(point) }
    }

    /// Plane through three points, `None` if they are (nearly) collinear.
    pub fn from_points(a: DVec3, b: DVec3, c: DVec3) -> Option<Self> {
        let n = (b - a).cross(c - a);
        if n.length() < TOL * TOL {
            return None;
        }
        Some(Self::new(a, n))
    }

    /// Best-fit plane for a closed polygon using Newell's method.
    pub fn from_polygon(pts: &[DVec3]) -> Option<Self> {
        if pts.len() < 3 {
            return None;
        }
        let mut n = DVec3::ZERO;
        let mut centroid = DVec3::ZERO;
        for i in 0..pts.len() {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            n.x += (p.y - q.y) * (p.z + q.z);
            n.y += (p.z - q.z) * (p.x + q.x);
            n.z += (p.x - q.x) * (p.y + q.y);
            centroid += p;
        }
        if n.length() < 1e-12 {
            return None;
        }
        Some(Self::new(centroid / pts.len() as f64, n))
    }

    #[inline]
    pub fn signed_distance(&self, p: DVec3) -> f64 {
        self.normal.dot(p) - self.d
    }

    #[inline]
    pub fn contains(&self, p: DVec3) -> bool {
        self.signed_distance(p).abs() <= TOL
    }

    pub fn project(&self, p: DVec3) -> DVec3 {
        p - self.normal * self.signed_distance(p)
    }

    pub fn origin(&self) -> DVec3 {
        self.normal * self.d
    }

    pub fn flipped(&self) -> Self {
        Self { normal: -self.normal, d: -self.d }
    }

    /// Same geometric plane regardless of orientation.
    pub fn coincident(&self, other: &Plane) -> bool {
        let dot = self.normal.dot(other.normal);
        if dot > 1.0 - ANGLE_TOL {
            approx_eq(self.d, other.d)
        } else if dot < -1.0 + ANGLE_TOL {
            approx_eq(self.d, -other.d)
        } else {
            false
        }
    }

    /// Orthonormal in-plane basis `(u, v)` such that `u × v = normal`.
    pub fn basis(&self) -> (DVec3, DVec3) {
        let n = self.normal;
        // Prefer world-aligned axes so 2D coordinates are stable and readable.
        let u = if n.z.abs() > 0.999 {
            DVec3::X * n.z.signum()
        } else {
            DVec3::Z.cross(n).normalize()
        };
        let v = n.cross(u);
        (u, v)
    }

    pub fn to_2d(&self, p: DVec3) -> DVec2 {
        let (u, v) = self.basis();
        let o = self.origin();
        DVec2::new((p - o).dot(u), (p - o).dot(v))
    }

    pub fn to_3d(&self, p: DVec2) -> DVec3 {
        let (u, v) = self.basis();
        self.origin() + u * p.x + v * p.y
    }

    pub fn intersect_ray(&self, ray: &Ray) -> Option<f64> {
        let denom = self.normal.dot(ray.dir);
        if denom.abs() < 1e-12 {
            return None;
        }
        Some((self.d - self.normal.dot(ray.origin)) / denom)
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: DVec3,
    /// Unit direction.
    pub dir: DVec3,
}

impl Ray {
    pub fn new(origin: DVec3, dir: DVec3) -> Self {
        Self { origin, dir: dir.normalize() }
    }

    pub fn at(&self, t: f64) -> DVec3 {
        self.origin + self.dir * t
    }

    /// Closest approach between the ray (as a line) and segment `a..b`.
    /// Returns `(t_ray, s_segment in 0..=1, distance)`.
    pub fn closest_to_segment(&self, a: DVec3, b: DVec3) -> (f64, f64, f64) {
        let (t, s) = closest_params_line_line(self.origin, self.dir, a, b - a);
        let s = s.clamp(0.0, 1.0);
        let q = a + (b - a) * s;
        // Recompute t for the clamped segment point.
        let t2 = (q - self.origin).dot(self.dir);
        let _ = t;
        let p = self.at(t2);
        (t2, s, p.distance(q))
    }

    /// Closest approach between the ray and the infinite line `p + d*k`.
    /// Returns `(t_ray, point_on_line)`.
    pub fn closest_to_line(&self, p: DVec3, d: DVec3) -> Option<(f64, DVec3)> {
        if parallel(self.dir, d) {
            return None;
        }
        let (t, s) = closest_params_line_line(self.origin, self.dir, p, d);
        Some((t, p + d * s))
    }
}

/// Parameters `(t, s)` of the closest points between lines `p1 + d1 t` and `p2 + d2 s`.
pub fn closest_params_line_line(p1: DVec3, d1: DVec3, p2: DVec3, d2: DVec3) -> (f64, f64) {
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let b = d1.dot(d2);
    let c = d1.dot(r);
    let denom = a * e - b * b;
    if denom.abs() < 1e-18 {
        return (0.0, if e > 0.0 { f / e } else { 0.0 });
    }
    let t = (b * f - c * e) / denom;
    let s = (a * f - b * c) / denom;
    (t, s)
}

/// Parameter (0..=1) and distance of the closest point on segment `a..b` to `p`.
pub fn closest_on_segment(p: DVec3, a: DVec3, b: DVec3) -> (f64, f64) {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-24 {
        return (0.0, p.distance(a));
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (t, p.distance(a + ab * t))
}

/// Closest point on the infinite line through `a` with direction `d`.
pub fn project_on_line(p: DVec3, a: DVec3, d: DVec3) -> DVec3 {
    let d = d.normalize();
    a + d * (p - a).dot(d)
}

/// Intersection of two 3D segments if they cross (within tolerance).
/// Returns `(s on first, t on second)` in `0..=1`.
pub fn segment_intersection(a0: DVec3, a1: DVec3, b0: DVec3, b1: DVec3) -> Option<(f64, f64)> {
    let da = a1 - a0;
    let db = b1 - b0;
    if parallel(da, db) {
        return None;
    }
    let (s, t) = closest_params_line_line(a0, da, b0, db);
    let la = da.length();
    let lb = db.length();
    let es = TOL / la;
    let et = TOL / lb;
    if s < -es || s > 1.0 + es || t < -et || t > 1.0 + et {
        return None;
    }
    let s = s.clamp(0.0, 1.0);
    let t = t.clamp(0.0, 1.0);
    let pa = a0 + da * s;
    let pb = b0 + db * t;
    if pa.distance(pb) > TOL {
        return None;
    }
    Some((s, t))
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: DVec3::splat(f64::INFINITY), max: DVec3::splat(f64::NEG_INFINITY) };

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    pub fn extend(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    pub fn diagonal(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.min.distance(self.max) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_roundtrip_2d() {
        let pl = Plane::new(DVec3::new(1.0, 2.0, 3.0), DVec3::new(1.0, 1.0, 1.0));
        let p = pl.project(DVec3::new(5.0, -3.0, 2.0));
        let q = pl.to_3d(pl.to_2d(p));
        assert!(points_eq(p, q));
    }

    #[test]
    fn ground_plane_basis_is_world_xy() {
        let pl = Plane::new(DVec3::ZERO, DVec3::Z);
        let (u, v) = pl.basis();
        assert!(points_eq(u, DVec3::X));
        assert!(points_eq(v, DVec3::Y));
    }

    #[test]
    fn segments_cross() {
        let r = segment_intersection(
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(5.0, -5.0, 0.0),
            DVec3::new(5.0, 5.0, 0.0),
        );
        let (s, t) = r.unwrap();
        assert!((s - 0.5).abs() < 1e-9 && (t - 0.5).abs() < 1e-9);
        assert!(
            segment_intersection(DVec3::ZERO, DVec3::X, DVec3::new(0.0, 1.0, 0.0), DVec3::new(1.0, 1.0, 0.0))
                .is_none()
        );
    }

    #[test]
    fn newell_plane() {
        let pts = [DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y];
        let pl = Plane::from_polygon(&pts).unwrap();
        assert!(points_eq(pl.normal, DVec3::Z));
    }
}
