//! A small sample model built with the same kernel operations the tools use.
//! Used by `--demo` and by tests as an end-to-end smoke check of the geometry kernel.

use sc_geom::{Mesh, PushPullMode};
use sc_math::DVec3;
use std::collections::HashSet;

const MM: f64 = 1.0 / 25.4;

fn face_with_normal(m: &Mesh, n: DVec3, at: impl Fn(&sc_geom::Face) -> bool) -> Option<sc_geom::FaceId> {
    m.faces.iter().find(|(_, f)| f.plane.normal.dot(n) > 0.999 && at(f)).map(|(id, _)| id)
}

/// A gable-roofed house with a recessed door and a round window.
pub fn house() -> Mesh {
    let mut m = Mesh::new();
    let (w, d, h, ridge) = (6000.0 * MM, 4000.0 * MM, 2700.0 * MM, 1500.0 * MM);
    let p = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);

    // Footprint, pulled up into a box.
    m.add_polyline(&[p(0.0, 0.0, 0.0), p(w, 0.0, 0.0), p(w, d, 0.0), p(0.0, d, 0.0)], true, false);
    let floor = m.faces.keys().next().unwrap();
    m.push_pull(floor, -h, PushPullMode::Auto);

    // Ridge line across the top, then lift it into a gable.
    m.add_line(p(0.0, d / 2.0, h), p(w, d / 2.0, h));
    let ridge_verts: HashSet<_> = m.vertices.iter().filter(|(_, v)| (v.pos.y - d / 2.0).abs() < 1e-6 && (v.pos.z - h).abs() < 1e-6).map(|(id, _)| id).collect();
    m.move_vertices(&ridge_verts, DVec3::Z * ridge);

    // Door on the front wall (y = 0, facing -Y), pushed in 150 mm.
    let (dx, dw, dh) = (1200.0 * MM, 1000.0 * MM, 2100.0 * MM);
    m.add_polyline(&[p(dx, 0.0, 0.0), p(dx + dw, 0.0, 0.0), p(dx + dw, 0.0, dh), p(dx, 0.0, dh)], true, false);
    if let Some(door) = face_with_normal(&m, -DVec3::Y, |f| f.outer.len() == 4 && f.holes.is_empty()) {
        m.push_pull(door, -150.0 * MM, PushPullMode::Auto);
    }

    // Round window.
    let c = p(4200.0 * MM, 0.0, 1500.0 * MM);
    let r = 500.0 * MM;
    let pts: Vec<DVec3> = (0..24)
        .map(|i| {
            let a = i as f64 / 24.0 * std::f64::consts::TAU;
            c + DVec3::new(a.cos() * r, 0.0, a.sin() * r)
        })
        .collect();
    m.add_polyline(&pts, true, true);
    m
}

/// An octagonal bush: a small prism, used as a component in the demo scene.
pub fn bush() -> Mesh {
    let mut m = Mesh::new();
    let r = 450.0 * MM;
    let pts: Vec<DVec3> = (0..8)
        .map(|i| {
            let a = (i as f64 + 0.5) / 8.0 * std::f64::consts::TAU;
            DVec3::new(r + a.cos() * r, r + a.sin() * r, 0.0)
        })
        .collect();
    m.add_polyline(&pts, true, true);
    let f = m.faces.keys().next().unwrap();
    m.push_pull(f, -900.0 * MM, PushPullMode::Auto);
    let green = Some([96, 150, 60, 255]);
    for face in m.faces.values_mut() {
        face.front = green;
        face.back = green;
    }
    m
}

#[cfg(test)]
mod tests {
    #[test]
    fn house_is_valid() {
        let m = super::house();
        m.validate().unwrap();
        // 4 walls + 2 roof planes + floor, plus door recess (3 sides + back) and window disc.
        assert!(m.faces.len() >= 11, "faces: {}", m.faces.len());
    }
}
