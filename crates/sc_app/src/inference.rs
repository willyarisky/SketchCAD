//! The inference ("snapping") engine.
//!
//! Given the cursor, it finds the most meaningful 3D point the user probably means, in SketchUp's
//! priority order: points (endpoint, midpoint, origin, guide point) beat linear inferences (axis
//! directions, from-point alignment, on edge), which beat "on face", which beats free space.

use crate::document::Guide;
use sc_geom::{EdgeId, FaceId, Mesh};
use sc_math::{DVec2, DVec3, Plane, Ray};
use sc_render::Camera;
use std::time::{Duration, Instant};

const POINT_PX: f64 = 10.0;
const LINE_PX: f64 = 8.0;
const AXIS_PX: f64 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Red,
    Green,
    Blue,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::Red, Axis::Green, Axis::Blue];

    pub fn dir(self) -> DVec3 {
        match self {
            Axis::Red => DVec3::X,
            Axis::Green => DVec3::Y,
            Axis::Blue => DVec3::Z,
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Axis::Red => egui::Color32::from_rgb(220, 0, 0),
            Axis::Green => egui::Color32::from_rgb(0, 160, 0),
            Axis::Blue => egui::Color32::from_rgb(0, 0, 230),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Axis::Red => "Red",
            Axis::Green => "Green",
            Axis::Blue => "Blue",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Free,
    Origin,
    Endpoint,
    Midpoint,
    GuidePoint,
    OnEdge,
    OnGuide,
    OnFace,
    OnAxis(Axis),
    /// Aligned with a remembered point along an axis.
    FromPoint(Axis),
    /// Locked with Shift / arrow keys.
    Locked(Option<Axis>),
}

impl Kind {
    pub fn tooltip(self) -> String {
        match self {
            Kind::Free => String::new(),
            Kind::Origin => "Origin".into(),
            Kind::Endpoint => "Endpoint".into(),
            Kind::Midpoint => "Midpoint".into(),
            Kind::GuidePoint => "Guide Point".into(),
            Kind::OnEdge => "On Edge".into(),
            Kind::OnGuide => "On Line".into(),
            Kind::OnFace => "On Face".into(),
            Kind::OnAxis(a) => format!("On {} Axis", a.name()),
            Kind::FromPoint(a) => format!("From Point ({})", a.name()),
            Kind::Locked(Some(a)) => format!("On {} Axis (locked)", a.name()),
            Kind::Locked(None) => "Locked".into(),
        }
    }

    pub fn is_point(self) -> bool {
        matches!(self, Kind::Origin | Kind::Endpoint | Kind::Midpoint | Kind::GuidePoint)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Inference {
    pub kind: Kind,
    pub point: DVec3,
    /// Edge / face in the active context's mesh (not set for geometry inside other contexts).
    pub edge: Option<EdgeId>,
    pub face: Option<FaceId>,
    /// Direction of the edge the point lies on, from any context.
    pub edge_dir: Option<DVec3>,
    /// Normal of the face the point lies on (or next to), from any context.
    pub face_normal: Option<DVec3>,
    /// For linear inferences: the anchor the dotted guide is drawn from.
    pub anchor: Option<DVec3>,
}

impl Inference {
    fn new(kind: Kind, point: DVec3) -> Self {
        Self { kind, point, edge: None, face: None, edge_dir: None, face_normal: None, anchor: None }
    }
}

/// A linear lock set with Shift (current inference) or arrow keys (axes).
#[derive(Clone, Copy, Debug)]
pub struct LineLock {
    pub origin: DVec3,
    pub dir: DVec3,
    pub axis: Option<Axis>,
}

pub struct Query<'a> {
    /// The context being edited.
    pub mesh: &'a Mesh,
    /// Everything else, flattened into the same coordinates (groups, components, outer model).
    pub extra: Option<&'a Mesh>,
    pub guides: &'a [Guide],
    pub camera: &'a Camera,
    pub size: DVec2,
    pub cursor: DVec2,
    /// Start point of the current operation (enables axis inference).
    pub from: Option<DVec3>,
    /// Drawing plane for free-space points (e.g. the face a rectangle started on).
    pub plane: Option<Plane>,
}

#[derive(Default)]
pub struct InferenceEngine {
    pub lock: Option<LineLock>,
    memory: Vec<(DVec3, Instant)>,
    hover: Option<(DVec3, Instant)>,
    pub last: Option<Inference>,
}

impl InferenceEngine {
    pub fn reset(&mut self) {
        self.lock = None;
        self.last = None;
    }

    pub fn lock_axis(&mut self, from: DVec3, axis: Axis) {
        if self.lock.is_some_and(|l| l.axis == Some(axis)) {
            self.lock = None;
        } else {
            self.lock = Some(LineLock { origin: from, dir: axis.dir(), axis: Some(axis) });
        }
    }

    /// Shift: lock whatever linear inference is current.
    pub fn lock_current(&mut self, from: Option<DVec3>) {
        let Some(last) = self.last else { return };
        let Some(from) = from else { return };
        let dir = last.point - from;
        if dir.length() < sc_math::TOL {
            return;
        }
        let axis = match last.kind {
            Kind::OnAxis(a) | Kind::Locked(Some(a)) => Some(a),
            _ => None,
        };
        self.lock = Some(LineLock { origin: from, dir: dir.normalize(), axis });
    }

    pub fn unlock(&mut self) {
        self.lock = None;
    }

    fn screen(q: &Query, p: DVec3) -> Option<DVec2> {
        q.camera.project(p, q.size)
    }

    fn visible(q: &Query, p: DVec3) -> bool {
        let origin = if q.camera.perspective { q.camera.eye } else { p - q.camera.forward() * (q.camera.distance() * 10.0) };
        let to = p - origin;
        let dist = to.length();
        if dist < 1e-9 {
            return true;
        }
        let ray = Ray::new(origin, to);
        match Self::pick_face(q, &ray) {
            Some((t, _, _)) => t >= dist - (dist * 1e-6).max(sc_math::TOL * 2.0),
            None => true,
        }
    }

    fn meshes<'m>(q: &Query<'m>) -> impl Iterator<Item = (bool, &'m Mesh)> {
        std::iter::once((true, q.mesh)).chain(q.extra.map(|m| (false, m)))
    }

    /// Nearest face hit across all meshes: (t, normal, face id if in the active mesh).
    fn pick_face(q: &Query, ray: &Ray) -> Option<(f64, DVec3, Option<FaceId>)> {
        let mut best: Option<(f64, DVec3, Option<FaceId>)> = None;
        for (primary, m) in Self::meshes(q) {
            if let Some(hit) = m.pick_face(ray, 0.0)
                && best.is_none_or(|b| hit.t < b.0) {
                    best = Some((hit.t, m.faces[hit.face].plane.normal, primary.then_some(hit.face)));
                }
        }
        best
    }

    fn screen_dist_to_segment(q: &Query, a: DVec3, b: DVec3) -> Option<f64> {
        let (sa, sb) = (Self::screen(q, a)?, Self::screen(q, b)?);
        let ab = sb - sa;
        let t = if ab.length_squared() > 0.0 { ((q.cursor - sa).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        Some(q.cursor.distance(sa + ab * t))
    }

    /// Screen distance from the cursor to the infinite 3D line through `p` with direction `d`.
    fn screen_dist_to_line(q: &Query, p: DVec3, d: DVec3) -> Option<(f64, DVec3)> {
        let ray = q.camera.ray(q.cursor, q.size);
        let (_, on_line) = ray.closest_to_line(p, d)?;
        let s = Self::screen(q, on_line)?;
        Some((s.distance(q.cursor), on_line))
    }

    pub fn infer(&mut self, q: &Query) -> Inference {
        let r = self.infer_inner(q);
        // Remember points the user hovers on for ~1/2 second; they seed from-point inferences.
        if r.kind.is_point() {
            match self.hover {
                Some((p, t)) if sc_math::points_eq(p, r.point) => {
                    if t.elapsed() > Duration::from_millis(400) && !self.memory.iter().any(|(m, _)| sc_math::points_eq(*m, p)) {
                        self.memory.push((p, Instant::now()));
                        if self.memory.len() > 4 {
                            self.memory.remove(0);
                        }
                    }
                }
                _ => self.hover = Some((r.point, Instant::now())),
            }
        } else {
            self.hover = None;
        }
        self.memory.retain(|(_, t)| t.elapsed() < Duration::from_secs(30));
        self.last = Some(r);
        r
    }

    fn infer_inner(&self, q: &Query) -> Inference {
        let ray = q.camera.ray(q.cursor, q.size);

        // 1. Point inferences.
        let mut best_pt: Option<(f64, Inference)> = None;
        let consider = |d: f64, inf: Inference, best: &mut Option<(f64, Inference)>| {
            if d <= POINT_PX && best.as_ref().is_none_or(|(bd, _)| d < *bd) && Self::visible(q, inf.point) {
                *best = Some((d, inf));
            }
        };
        for (primary, mesh) in Self::meshes(q) {
            for (_, v) in &mesh.vertices {
                if let Some(s) = Self::screen(q, v.pos) {
                    consider(s.distance(q.cursor), Inference::new(Kind::Endpoint, v.pos), &mut best_pt);
                }
            }
            for (eid, e) in &mesh.edges {
                let (a, b) = (mesh.pos(e.v[0]), mesh.pos(e.v[1]));
                let m = (a + b) * 0.5;
                if let Some(s) = Self::screen(q, m) {
                    let mut inf = Inference::new(Kind::Midpoint, m);
                    inf.edge = primary.then_some(eid);
                    inf.edge_dir = Some((b - a).normalize());
                    // Endpoints win ties over midpoints.
                    consider(s.distance(q.cursor) + 1.0, inf, &mut best_pt);
                }
            }
        }
        for g in q.guides {
            if let Guide::Point { point } = g
                && let Some(s) = Self::screen(q, *point) {
                    consider(s.distance(q.cursor), Inference::new(Kind::GuidePoint, *point), &mut best_pt);
                }
        }
        if let Some(s) = Self::screen(q, DVec3::ZERO) {
            consider(s.distance(q.cursor) + 0.5, Inference::new(Kind::Origin, DVec3::ZERO), &mut best_pt);
        }

        // 2. Locked direction: everything projects onto the lock line.
        if let Some(lock) = self.lock {
            let p = match best_pt {
                Some((_, inf)) => sc_math::project_on_line(inf.point, lock.origin, lock.dir),
                None => ray.closest_to_line(lock.origin, lock.dir).map(|(_, p)| p).unwrap_or(lock.origin),
            };
            let mut inf = Inference::new(Kind::Locked(lock.axis), p);
            inf.anchor = Some(lock.origin);
            return inf;
        }
        if let Some((_, inf)) = best_pt {
            return inf;
        }

        // 3. Linear inferences: axes through the start point and through remembered points.
        let mut linear: Vec<(f64, DVec3, Axis, DVec3, bool)> = Vec::new(); // (px, point, axis, anchor, from_start)
        if let Some(from) = q.from {
            for a in Axis::ALL {
                if let Some((d, p)) = Self::screen_dist_to_line(q, from, a.dir())
                    && d <= AXIS_PX && (p - from).length() > sc_math::TOL {
                        linear.push((d, p, a, from, true));
                    }
            }
        }
        for (m, _) in &self.memory {
            if q.from.is_some_and(|f| sc_math::points_eq(f, *m)) {
                continue;
            }
            for a in Axis::ALL {
                if let Some((d, p)) = Self::screen_dist_to_line(q, *m, a.dir())
                    && d <= AXIS_PX * 0.6 {
                        linear.push((d, p, a, *m, false));
                    }
            }
        }
        linear.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // Intersection of an axis from the start point with an alignment from a remembered point.
        if let (Some(s), Some(m)) = (linear.iter().find(|l| l.4), linear.iter().find(|l| !l.4))
            && !sc_math::parallel(s.2.dir(), m.2.dir()) {
                let (t, _) = sc_math::closest_params_line_line(s.3, s.2.dir(), m.3, m.2.dir());
                let p = s.3 + s.2.dir() * t;
                if let Some(sp) = Self::screen(q, p)
                    && sp.distance(q.cursor) <= AXIS_PX * 1.5 {
                        let mut inf = Inference::new(Kind::FromPoint(m.2), p);
                        inf.anchor = Some(m.3);
                        return inf;
                    }
            }
        if let Some(&(_, p, a, anchor, from_start)) = linear.first() {
            let mut inf = Inference::new(if from_start { Kind::OnAxis(a) } else { Kind::FromPoint(a) }, p);
            inf.anchor = Some(anchor);
            return inf;
        }

        // 4. On edge / on guide line.
        let mut best_edge: Option<(f64, Inference)> = None;
        for (primary, mesh) in Self::meshes(q) {
            for (eid, e) in &mesh.edges {
                if e.hidden {
                    continue;
                }
                let (a, b) = (mesh.pos(e.v[0]), mesh.pos(e.v[1]));
                let Some(d) = Self::screen_dist_to_segment(q, a, b) else { continue };
                if d > LINE_PX || best_edge.as_ref().is_some_and(|(bd, _)| d >= *bd) {
                    continue;
                }
                let (_, s, _) = ray.closest_to_segment(a, b);
                let p = a + (b - a) * s;
                if Self::visible(q, p) {
                    let mut inf = Inference::new(Kind::OnEdge, p);
                    inf.edge = primary.then_some(eid);
                    inf.edge_dir = Some((b - a).normalize());
                    inf.face_normal = e.faces.first().map(|f| mesh.faces[*f].plane.normal);
                    best_edge = Some((d, inf));
                }
            }
        }
        if let Some((_, inf)) = best_edge {
            return inf;
        }
        for g in q.guides {
            if let Guide::Line { point, dir } = g
                && let Some((d, p)) = Self::screen_dist_to_line(q, *point, *dir)
                    && d <= LINE_PX * 0.75 && Self::visible(q, p) {
                        return Inference::new(Kind::OnGuide, p);
                    }
        }

        // 5. On face.
        if let Some((t, normal, face)) = Self::pick_face(q, &ray) {
            let mut inf = Inference::new(Kind::OnFace, ray.at(t));
            inf.face = face;
            inf.face_normal = Some(normal);
            return inf;
        }

        // 6. Free space.
        Inference::new(Kind::Free, free_point(q, &ray))
    }
}

/// Where a point lands with nothing to snap to: on the drawing plane, else the ground.
pub fn free_point(q: &Query, ray: &Ray) -> DVec3 {
    let mut planes = Vec::new();
    if let Some(pl) = q.plane {
        planes.push(pl);
    }
    if let Some(f) = q.from {
        planes.push(Plane::new(f, DVec3::Z));
        planes.push(Plane::new(f, -q.camera.forward()));
    } else {
        planes.push(Plane::new(DVec3::ZERO, DVec3::Z));
    }
    for pl in planes {
        if let Some(t) = pl.intersect_ray(ray)
            && t > 0.0 && t < q.camera.distance() * 50.0 {
                return ray.at(t);
            }
    }
    // Looking at the sky: a point in front of the camera at target distance.
    ray.at(q.camera.distance())
}
