//! Drawing tools: Line, Rectangle, Circle / Polygon, 2 Point Arc.

use super::*;
use sc_math::units::VcbInput;

fn tooltip_at(ov: &mut Overlay, cx: &ToolCx, text: &str) {
    let p = egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32) + egui::vec2(14.0, 18.0);
    ov.tooltips.push((p, text.to_string()));
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct LineTool {
    start: Option<DVec3>,
    cur: DVec3,
    press_started_chain: bool,
}

impl LineTool {
    fn commit(&mut self, cx: &mut ToolCx, end: DVec3) {
        let Some(s) = self.start else { return };
        if sc_math::points_eq(s, end) {
            return;
        }
        cx.checkpoint("Line");
        let faces_before = cx.mesh.faces.len();
        cx.mesh.add_line(s, end);
        cx.inference.unlock();
        // Like SketchUp, keep drawing from the end point unless the line closed a face.
        if cx.mesh.faces.len() > faces_before {
            self.start = None;
        } else {
            self.start = Some(end);
        }
        cx.vcb.value = cx.fmt_len(s.distance(end));
    }
}

impl Tool for LineTool {
    fn id(&self) -> ToolId {
        ToolId::Line
    }
    fn status(&self) -> String {
        if self.start.is_none() {
            "Select start point.".into()
        } else {
            "Select end point or enter value. Arrows = lock axis, Shift = lock inference.".into()
        }
    }
    fn vcb_label(&self) -> &'static str {
        "Length"
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let inf = cx.infer(self.start, None);
        self.cur = inf.point;
        if let Some(s) = self.start {
            cx.vcb.value = cx.fmt_len(s.distance(self.cur));
        }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        self.on_move(cx);
        if self.start.is_none() {
            self.start = Some(self.cur);
            self.press_started_chain = true;
        } else {
            self.press_started_chain = false;
            self.commit(cx, self.cur);
        }
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        // Click-drag-release draws a single segment.
        if dragged && self.press_started_chain {
            self.on_move(cx);
            self.commit(cx, self.cur);
        }
        self.press_started_chain = false;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        match cx.vcb_input(text) {
            Some(VcbInput::Absolute(p)) => {
                let p = DVec3::from_array(p);
                if self.start.is_none() {
                    self.start = Some(p);
                } else {
                    self.commit(cx, p);
                }
                Ok(())
            }
            Some(VcbInput::Relative(d)) => {
                let s = self.start.ok_or("Pick a start point first")?;
                self.commit(cx, s + DVec3::from_array(d));
                Ok(())
            }
            Some(VcbInput::Lengths(v)) if v.len() == 1 => {
                let s = self.start.ok_or("Pick a start point first")?;
                let dir = self.cur - s;
                if dir.length() < sc_math::TOL {
                    return Err("Move the mouse to set a direction".into());
                }
                self.commit(cx, s + dir.normalize() * v[0]);
                Ok(())
            }
            _ => Err("Invalid length".into()),
        }
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        self.start = None;
        cx.inference.unlock();
    }
    fn draw(&self, _cx: &ToolCx, ov: &mut Overlay) {
        if let Some(s) = self.start {
            ov.lines.push(line(s, self.cur, axis_color(s, self.cur), 2.0));
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        self.start
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct RectFrame {
    origin: DVec3,
    plane: Plane,
    u: DVec3,
    v: DVec3,
}

#[derive(Default)]
pub struct RectangleTool {
    frame: Option<RectFrame>,
    size: (f64, f64),
    snap: Option<&'static str>,
    /// For re-typing dimensions right after drawing.
    last: Option<(sc_geom::Mesh, RectFrame)>,
    dragging_from_press: bool,
}

impl RectangleTool {
    fn corners(f: &RectFrame, w: f64, h: f64) -> [DVec3; 4] {
        [f.origin, f.origin + f.u * w, f.origin + f.u * w + f.v * h, f.origin + f.v * h]
    }

    fn commit(&mut self, cx: &mut ToolCx, w: f64, h: f64) {
        let Some(f) = self.frame.take() else { return };
        if w.abs() < sc_math::TOL || h.abs() < sc_math::TOL {
            return;
        }
        let base = cx.mesh.clone();
        cx.checkpoint_with("Rectangle", base.clone());
        cx.mesh.add_polyline(&Self::corners(&f, w, h), true, false);
        self.last = Some((base, f));
        cx.vcb.value = format!("{}, {}", cx.fmt_len(w.abs()), cx.fmt_len(h.abs()));
    }
}

impl Tool for RectangleTool {
    fn id(&self) -> ToolId {
        ToolId::Rectangle
    }
    fn status(&self) -> String {
        if self.frame.is_none() { "Select first corner.".into() } else { "Select opposite corner or enter value.".into() }
    }
    fn vcb_label(&self) -> &'static str {
        "Dimensions"
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(f) = self.frame else {
            cx.infer(None, None);
            return;
        };
        let (p, _) = point_on_plane(cx, &f.plane, f.origin);
        let d = p - f.origin;
        let (w, mut h) = (d.dot(f.u), d.dot(f.v));
        let upp = cx.camera.units_per_pixel(p, cx.size.y);
        self.snap = None;
        let golden = std::f64::consts::GOLDEN_RATIO;
        if (w.abs() - h.abs()).abs() < 6.0 * upp && w.abs() > 10.0 * upp {
            h = h.signum() * w.abs();
            self.snap = Some("Square");
        } else if (h.abs() - w.abs() / golden).abs() < 6.0 * upp && w.abs() > 10.0 * upp {
            h = h.signum() * w.abs() / golden;
            self.snap = Some("Golden Section");
        } else if (h.abs() - w.abs() * golden).abs() < 6.0 * upp && w.abs() > 10.0 * upp {
            h = h.signum() * w.abs() * golden;
            self.snap = Some("Golden Section");
        }
        self.size = (w, h);
        cx.vcb.value = format!("{}, {}", cx.fmt_len(w.abs()), cx.fmt_len(h.abs()));
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.frame.is_none() {
            let inf = cx.infer(None, None);
            let plane = drawing_plane(cx, &inf);
            let (u, v) = plane_axes(plane.normal);
            self.frame = Some(RectFrame { origin: inf.point, plane, u, v });
            self.size = (0.0, 0.0);
            self.dragging_from_press = true;
        } else {
            self.on_move(cx);
            let (w, h) = self.size;
            self.commit(cx, w, h);
            self.dragging_from_press = false;
        }
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.dragging_from_press {
            self.on_move(cx);
            let (w, h) = self.size;
            self.commit(cx, w, h);
        }
        self.dragging_from_press = false;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let Some(VcbInput::Lengths(v)) = cx.vcb_input(text) else { return Err("Enter width,height".into()) };
        let (w, h) = match v.as_slice() {
            [w, h] => (*w, *h),
            [w] => (*w, *w),
            _ => return Err("Enter width,height".into()),
        };
        if self.frame.is_some() {
            // Keep the sign of the current mouse direction.
            let (cw, ch) = self.size;
            let w = if cw < 0.0 { -w.abs() } else { w };
            let h = if ch < 0.0 { -h.abs() } else { h };
            self.commit(cx, w, h);
            return Ok(());
        }
        // Re-size the rectangle that was just drawn.
        let Some((base, f)) = self.last.take() else { return Err("Draw a rectangle first".into()) };
        *cx.mesh = base.clone();
        cx.mesh.revision += 1;
        cx.mesh.add_polyline(&Self::corners(&f, w, h), true, false);
        self.last = Some((base, f));
        cx.vcb.value = format!("{}, {}", cx.fmt_len(w.abs()), cx.fmt_len(h.abs()));
        Ok(())
    }
    fn cancel(&mut self, _cx: &mut ToolCx) {
        self.frame = None;
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        let Some(f) = self.frame else { return };
        let c = Self::corners(&f, self.size.0, self.size.1);
        for i in 0..4 {
            ov.lines.push(line(c[i], c[(i + 1) % 4], axis_color(c[i], c[(i + 1) % 4]), 2.0));
        }
        if let Some(s) = self.snap {
            // Diagonal shows the square / golden-section inference.
            ov.lines.push(line(c[0], c[2], egui::Color32::from_rgb(0, 120, 255), 1.0).dashed(4.0));
            tooltip_at(ov, cx, s);
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        self.frame.map(|f| f.origin)
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct CircleParams {
    center: DVec3,
    normal: DVec3,
    dir: DVec3,
    radius: f64,
    sides: u32,
}

pub struct CircleTool {
    polygon: bool,
    sides: u32,
    center: Option<(DVec3, Plane)>,
    cur: DVec3,
    last: Option<(sc_geom::Mesh, CircleParams)>,
    dragging_from_press: bool,
}

impl CircleTool {
    pub fn new(polygon: bool) -> Self {
        Self { polygon, sides: if polygon { 6 } else { 24 }, center: None, cur: DVec3::ZERO, last: None, dragging_from_press: false }
    }

    fn points(p: &CircleParams) -> Vec<DVec3> {
        let w = p.normal.cross(p.dir).normalize();
        (0..p.sides)
            .map(|i| {
                let a = i as f64 / p.sides as f64 * std::f64::consts::TAU;
                p.center + (p.dir * a.cos() + w * a.sin()) * p.radius
            })
            .collect()
    }

    fn params(&self, radius: Option<f64>) -> Option<CircleParams> {
        let (c, pl) = self.center?;
        let d = self.cur - c;
        let dir = if d.length() > sc_math::TOL { d.normalize() } else { plane_axes(pl.normal).0 };
        Some(CircleParams { center: c, normal: pl.normal, dir, radius: radius.unwrap_or(d.length()), sides: self.sides })
    }

    fn commit(&mut self, cx: &mut ToolCx, p: CircleParams) {
        self.center = None;
        if p.radius < sc_math::TOL {
            return;
        }
        let base = cx.mesh.clone();
        cx.checkpoint_with(if self.polygon { "Polygon" } else { "Circle" }, base.clone());
        cx.mesh.add_polyline(&Self::points(&p), true, true);
        self.last = Some((base, p));
        cx.vcb.value = cx.fmt_len(p.radius);
    }

    fn redo_last(&mut self, cx: &mut ToolCx, f: impl Fn(&mut CircleParams)) -> bool {
        let Some((base, mut p)) = self.last.take() else { return false };
        f(&mut p);
        *cx.mesh = base.clone();
        cx.mesh.revision += 1;
        cx.mesh.add_polyline(&Self::points(&p), true, true);
        self.last = Some((base, p));
        true
    }
}

impl Tool for CircleTool {
    fn id(&self) -> ToolId {
        if self.polygon { ToolId::Polygon } else { ToolId::Circle }
    }
    fn status(&self) -> String {
        if self.center.is_none() { "Select center point.".into() } else { "Select edge of circle or enter radius.".into() }
    }
    fn vcb_label(&self) -> &'static str {
        if self.center.is_none() { "Sides" } else { "Radius" }
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        match self.center {
            None => {
                cx.infer(None, None);
                cx.vcb.value = self.sides.to_string();
            }
            Some((c, pl)) => {
                let (p, _) = point_on_plane(cx, &pl, c);
                self.cur = p;
                cx.vcb.value = cx.fmt_len(c.distance(p));
            }
        }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.center.is_none() {
            let inf = cx.infer(None, None);
            self.center = Some((inf.point, drawing_plane(cx, &inf)));
            self.cur = inf.point;
            self.dragging_from_press = true;
        } else {
            self.on_move(cx);
            if let Some(p) = self.params(None) {
                self.commit(cx, p);
            }
            self.dragging_from_press = false;
        }
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.dragging_from_press {
            self.on_move(cx);
            if let Some(p) = self.params(None) {
                self.commit(cx, p);
            }
        }
        self.dragging_from_press = false;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        // Before the center is picked the box reads "Sides": a bare number sets the side count.
        if self.center.is_none() && self.last.is_none()
            && let Ok(n) = text.trim().trim_end_matches('s').parse::<u32>() {
                return self.set_sides(n);
            }
        match cx.vcb_input(text) {
            Some(VcbInput::Sides(n)) => {
                self.set_sides(n)?;
                if self.center.is_none() {
                    self.redo_last(cx, |p| p.sides = n);
                }
                Ok(())
            }
            Some(VcbInput::Lengths(v)) if v.len() == 1 => {
                if let Some(p) = self.params(Some(v[0])) {
                    self.commit(cx, p);
                    Ok(())
                } else if self.redo_last(cx, |p| p.radius = v[0]) {
                    cx.vcb.value = cx.fmt_len(v[0]);
                    Ok(())
                } else {
                    Err("Select a center point first".into())
                }
            }
            _ => Err("Invalid value".into()),
        }
    }
    fn cancel(&mut self, _cx: &mut ToolCx) {
        self.center = None;
    }
    fn draw(&self, _cx: &ToolCx, ov: &mut Overlay) {
        let Some(p) = self.params(None) else { return };
        if p.radius < sc_math::TOL {
            return;
        }
        let pts = Self::points(&p);
        for i in 0..pts.len() {
            ov.lines.push(line(pts[i], pts[(i + 1) % pts.len()], egui::Color32::BLACK, 2.0));
        }
        ov.lines.push(line(p.center, self.cur, egui::Color32::from_gray(80), 1.0).dashed(4.0));
    }
    fn start_point(&self) -> Option<DVec3> {
        self.center.map(|c| c.0)
    }
}

impl CircleTool {
    fn set_sides(&mut self, n: u32) -> Result<(), String> {
        if !(3..=999).contains(&n) {
            return Err("Number of sides must be between 3 and 999".into());
        }
        self.sides = n;
        Ok(())
    }
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ArcTool {
    start: Option<(DVec3, Plane)>,
    end: Option<DVec3>,
    cur: DVec3,
    bulge: f64,
    segments: Option<u32>,
    half_circle: bool,
}

impl ArcTool {
    /// Points of an arc from `a` to `b` bulging `s` along the in-plane perpendicular.
    pub fn arc_points(a: DVec3, b: DVec3, normal: DVec3, s: f64, segs: u32) -> Vec<DVec3> {
        let chord = b - a;
        let c = chord.length();
        if c < sc_math::TOL {
            return vec![a, b];
        }
        if s.abs() < sc_math::TOL {
            return vec![a, b];
        }
        let perp = normal.cross(chord).normalize();
        let mid = (a + b) * 0.5;
        let r = (c * c / 4.0 + s * s) / (2.0 * s.abs());
        let center = mid + perp * s.signum() * (s.abs() - r);
        let apex = mid + perp * s;
        let e1 = (a - center).normalize();
        let mut e2 = normal.cross(e1).normalize();
        let ang = |p: DVec3, e2: DVec3| {
            let d = p - center;
            let t = d.dot(e2).atan2(d.dot(e1));
            if t < 0.0 { t + std::f64::consts::TAU } else { t }
        };
        if ang(apex, e2) > ang(b, e2) {
            e2 = -e2;
        }
        let sweep = ang(b, e2);
        (0..=segs)
            .map(|i| {
                let t = sweep * i as f64 / segs as f64;
                center + (e1 * t.cos() + e2 * t.sin()) * r
            })
            .collect()
    }

    fn default_segments(&self) -> u32 {
        self.segments.unwrap_or(12)
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let (Some((a, pl)), Some(b)) = (self.start, self.end) else { return };
        let pts = Self::arc_points(a, b, pl.normal, self.bulge, self.default_segments());
        cx.checkpoint("Arc");
        cx.mesh.add_polyline(&pts, false, true);
        self.start = None;
        self.end = None;
    }
}

impl Tool for ArcTool {
    fn id(&self) -> ToolId {
        ToolId::Arc
    }
    fn status(&self) -> String {
        match (self.start, self.end) {
            (None, _) => "Select start point.".into(),
            (Some(_), None) => "Select end point or enter length.".into(),
            _ => "Select bulge or enter value.".into(),
        }
    }
    fn vcb_label(&self) -> &'static str {
        if self.end.is_some() { "Bulge" } else { "Length" }
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        match (self.start, self.end) {
            (None, _) => {
                cx.infer(None, None);
            }
            (Some((a, pl)), None) => {
                let (p, _) = point_on_plane(cx, &pl, a);
                self.cur = p;
                cx.vcb.value = cx.fmt_len(a.distance(p));
            }
            (Some((a, pl)), Some(b)) => {
                let (p, _) = point_on_plane(cx, &pl, a);
                self.cur = p;
                let chord = b - a;
                let perp = pl.normal.cross(chord).normalize();
                let mut s = (p - (a + b) * 0.5).dot(perp);
                let half = chord.length() * 0.5;
                let upp = cx.camera.units_per_pixel(p, cx.size.y);
                self.half_circle = (s.abs() - half).abs() < 6.0 * upp;
                if self.half_circle {
                    s = s.signum() * half;
                }
                self.bulge = s;
                cx.vcb.value = cx.fmt_len(s.abs());
            }
        }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        match (self.start, self.end) {
            (None, _) => {
                let inf = cx.infer(None, None);
                self.start = Some((inf.point, drawing_plane(cx, &inf)));
                self.cur = inf.point;
            }
            (Some(_), None) => {
                self.on_move(cx);
                self.end = Some(self.cur);
                self.bulge = 0.0;
            }
            _ => {
                self.on_move(cx);
                self.commit(cx);
            }
        }
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        match cx.vcb_input(text) {
            Some(VcbInput::Sides(n)) if n >= 2 => {
                self.segments = Some(n);
                Ok(())
            }
            Some(VcbInput::Lengths(v)) if v.len() == 1 => match (self.start, self.end) {
                (Some((a, _)), None) => {
                    let d = self.cur - a;
                    if d.length() < sc_math::TOL {
                        return Err("Move the mouse to set a direction".into());
                    }
                    self.end = Some(a + d.normalize() * v[0]);
                    Ok(())
                }
                (Some(_), Some(_)) => {
                    self.bulge = if self.bulge < 0.0 { -v[0] } else { v[0] };
                    self.commit(cx);
                    Ok(())
                }
                _ => Err("Select a start point first".into()),
            },
            _ => Err("Invalid value".into()),
        }
    }
    fn cancel(&mut self, _cx: &mut ToolCx) {
        self.start = None;
        self.end = None;
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        match (self.start, self.end) {
            (Some((a, _)), None) => ov.lines.push(line(a, self.cur, axis_color(a, self.cur), 2.0)),
            (Some((a, pl)), Some(b)) => {
                let pts = Self::arc_points(a, b, pl.normal, self.bulge, self.default_segments());
                for w in pts.windows(2) {
                    ov.lines.push(line(w[0], w[1], egui::Color32::BLACK, 2.0));
                }
                ov.lines.push(line(a, b, egui::Color32::from_gray(120), 1.0).dashed(4.0));
                if self.half_circle {
                    tooltip_at(ov, cx, "Half Circle");
                }
            }
            _ => {}
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        match (self.start, self.end) {
            (Some((a, _)), None) => Some(a),
            _ => None,
        }
    }
}
