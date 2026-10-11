//! SketchUp-style camera: Z up, orbit around a pivot, pan, zoom toward the cursor.
//!
//! All math is f64 in model inches; matrices are converted to f32 only for the GPU.
//! Projection uses reversed-Z (near = 1, far = 0) for depth precision on large models.

use glam::dcamera::rh::{proj::directx, view::look_at_mat4};
use sc_math::{Aabb, DMat4, DVec2, DVec3, DVec4, Ray};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub eye: DVec3,
    pub target: DVec3,
    /// Vertical field of view in degrees (SketchUp default: 35°).
    pub fov_y: f64,
    pub perspective: bool,
    /// Explicit screen-up for cameras expressed in a group's local space (see [`Camera::to_local`]).
    #[serde(skip)]
    pub screen_up: Option<DVec3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    Iso,
}

impl Default for Camera {
    fn default() -> Self {
        // Roughly SketchUp's start-up view of a new template: looking at the origin from front-left, above.
        Self { eye: DVec3::new(-200.0, -350.0, 180.0), target: DVec3::new(60.0, 40.0, 30.0), fov_y: 35.0, perspective: true, screen_up: None }
    }
}

impl Camera {
    pub fn forward(&self) -> DVec3 {
        (self.target - self.eye).normalize()
    }

    pub fn up(&self) -> DVec3 {
        let f = self.forward();
        if let Some(u) = self.screen_up {
            return (u - f * u.dot(f)).normalize();
        }
        let right = f.cross(DVec3::Z);
        if right.length() < 1e-9 {
            // Looking straight up/down: use +Y as screen-up like SketchUp's Top view.
            return if f.z < 0.0 { DVec3::Y } else { -DVec3::Y };
        }
        right.normalize().cross(f).normalize()
    }

    pub fn right(&self) -> DVec3 {
        self.forward().cross(self.up()).normalize()
    }

    pub fn distance(&self) -> f64 {
        self.eye.distance(self.target)
    }

    pub fn view(&self) -> DMat4 {
        look_at_mat4(self.eye, self.target, self.up())
    }

    /// Visible half-height at the target distance (used for parallel projection and panning).
    pub fn half_height_at(&self, depth: f64) -> f64 {
        depth * (self.fov_y.to_radians() * 0.5).tan()
    }

    pub fn proj(&self, aspect: f64) -> DMat4 {
        if self.perspective {
            let near = (self.distance() * 1e-4).max(0.01);
            directx::perspective_infinite_reverse(self.fov_y.to_radians(), aspect, near)
        } else {
            let h = self.half_height_at(self.distance());
            let w = h * aspect;
            let range = self.distance() * 1000.0 + 1e5;
            // Reversed Z: swap near/far.
            directx::orthographic(-w, w, -h, h, range, -range)
        }
    }

    pub fn view_proj(&self, size: DVec2) -> DMat4 {
        self.proj(size.x / size.y.max(1.0)) * self.view()
    }

    /// Project a model point to viewport pixels (origin top-left). `None` if behind the camera.
    pub fn project(&self, p: DVec3, size: DVec2) -> Option<DVec2> {
        let c = self.view_proj(size) * DVec4::new(p.x, p.y, p.z, 1.0);
        if c.w <= 1e-9 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        Some(DVec2::new((ndc.x + 1.0) * 0.5 * size.x, (1.0 - ndc.y) * 0.5 * size.y))
    }

    /// Ray through a viewport pixel.
    pub fn ray(&self, px: DVec2, size: DVec2) -> Ray {
        let ndc = DVec2::new(px.x / size.x * 2.0 - 1.0, 1.0 - px.y / size.y * 2.0);
        let inv = self.view_proj(size).inverse();
        let unproject = |z: f64| {
            let v = inv * DVec4::new(ndc.x, ndc.y, z, 1.0);
            v.truncate() / v.w
        };
        if self.perspective {
            let far = unproject(0.5);
            Ray::new(self.eye, far - self.eye)
        } else {
            let a = unproject(1.0);
            let b = unproject(0.0);
            Ray::new(a, b - a)
        }
    }

    /// Model units per pixel at a given point (for pixel-tolerance picking and pan speed).
    pub fn units_per_pixel(&self, at: DVec3, viewport_h: f64) -> f64 {
        let depth = if self.perspective { (at - self.eye).dot(self.forward()).max(1e-6) } else { self.distance() };
        2.0 * self.half_height_at(depth) / viewport_h.max(1.0)
    }

    /// Orbit around `pivot` by screen-space pixel deltas.
    pub fn orbit(&mut self, dx: f64, dy: f64, pivot: DVec3) {
        let speed = 0.008;
        let yaw = DMat4::from_axis_angle(DVec3::Z, -dx * speed);
        let rot_about = |m: DMat4, p: DVec3, c: DVec3| m.transform_point3(p - c) + c;
        self.eye = rot_about(yaw, self.eye, pivot);
        self.target = rot_about(yaw, self.target, pivot);
        // Pitch, clamped so the camera never flips over the pole.
        let f = self.forward();
        let pitch_now = f.z.clamp(-1.0, 1.0).asin();
        let max = 89.5f64.to_radians();
        let new_pitch = (pitch_now + dy * speed).clamp(-max, max);
        let d = new_pitch - pitch_now;
        let right = self.right();
        let pitch = DMat4::from_axis_angle(right, d);
        self.eye = rot_about(pitch, self.eye, pivot);
        self.target = rot_about(pitch, self.target, pivot);
    }

    /// The same view expressed in the coordinates of a context whose local → world transform is
    /// `to_world` (rigid or uniformly scaled). Projecting local points with the result matches
    /// projecting their world positions with `self`.
    pub fn to_local(&self, to_world: &DMat4) -> Camera {
        if *to_world == DMat4::IDENTITY {
            return *self;
        }
        let inv = to_world.inverse();
        Camera {
            eye: inv.transform_point3(self.eye),
            target: inv.transform_point3(self.target),
            fov_y: self.fov_y,
            perspective: self.perspective,
            screen_up: Some(inv.transform_vector3(self.up()).normalize()),
        }
    }

    /// Pan so the point at depth `at` follows the cursor.
    pub fn pan(&mut self, dx: f64, dy: f64, at: DVec3, viewport_h: f64) {
        let upp = self.units_per_pixel(at, viewport_h);
        let delta = self.right() * (-dx * upp) + self.up() * (dy * upp);
        self.eye += delta;
        self.target += delta;
    }

    /// Zoom toward `toward` (point under the cursor). `steps > 0` zooms in.
    pub fn zoom(&mut self, steps: f64, toward: DVec3) {
        let factor = 1.0 - (0.85f64).powf(steps);
        if self.perspective {
            let to = toward - self.eye;
            // Never step through the point.
            let move_v = to * factor.min(0.9);
            self.eye += move_v;
            self.target += move_v;
            // Keep target at a sensible distance in front of the eye.
            let f = self.forward();
            let d = (toward - self.eye).dot(f).max(1.0);
            self.target = self.eye + f * d;
        } else {
            let scale = 1.0 - factor;
            let d = self.distance() * scale;
            let shift = (toward - self.target) * factor;
            let shift = shift - self.forward() * shift.dot(self.forward());
            let f = self.forward();
            self.target += shift;
            self.eye = self.target - f * d.max(0.01);
        }
    }

    /// Frame a bounding box (Zoom Extents).
    pub fn zoom_extents(&mut self, b: &Aabb, aspect: f64) {
        if b.is_empty() {
            *self = Camera { perspective: self.perspective, fov_y: self.fov_y, ..Camera::default() };
            return;
        }
        let r = (b.diagonal() * 0.5).max(1.0);
        let half_v = self.fov_y.to_radians() * 0.5;
        let half_h = (half_v.tan() * aspect).atan();
        let dist = r / half_v.min(half_h).sin() * 1.02;
        let f = self.forward();
        self.target = b.center();
        self.eye = self.target - f * dist;
    }

    pub fn set_standard_view(&mut self, v: StandardView, bounds: &Aabb, aspect: f64) {
        let dir = match v {
            StandardView::Top => DVec3::new(0.0, -1e-6, -1.0),
            StandardView::Bottom => DVec3::new(0.0, 1e-6, 1.0),
            StandardView::Front => DVec3::Y,
            StandardView::Back => -DVec3::Y,
            StandardView::Left => DVec3::X,
            StandardView::Right => -DVec3::X,
            StandardView::Iso => DVec3::new(1.0, 1.0, -0.8),
        }
        .normalize();
        let d = self.distance();
        self.eye = self.target - dir * d;
        self.zoom_extents(bounds, aspect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_ray_agree() {
        let cam = Camera::default();
        let size = DVec2::new(800.0, 600.0);
        let p = DVec3::new(10.0, 20.0, 5.0);
        let s = cam.project(p, size).unwrap();
        let ray = cam.ray(s, size);
        let (_, d) = sc_math::closest_on_segment(p, ray.origin, ray.at(1e5));
        assert!(d < 1e-6, "distance {d}");
    }

    #[test]
    fn local_camera_projects_like_world() {
        let cam = Camera::default();
        let size = DVec2::new(800.0, 600.0);
        let xf = DMat4::from_translation(DVec3::new(30.0, -10.0, 5.0)) * DMat4::from_rotation_y(0.7) * DMat4::from_rotation_z(1.1);
        let local = cam.to_local(&xf);
        let q = DVec3::new(3.0, 4.0, 5.0);
        let a = cam.project(xf.transform_point3(q), size).unwrap();
        let b = local.project(q, size).unwrap();
        assert!(a.distance(b) < 1e-6, "{a} vs {b}");
    }

    #[test]
    fn orbit_keeps_distance() {
        let mut cam = Camera::default();
        let d0 = cam.distance();
        cam.orbit(100.0, 40.0, cam.target);
        assert!((cam.distance() - d0).abs() < 1e-6);
    }
}
