use super::*;
use sc_math::DMat4;

fn p(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn rect(m: &mut Mesh, x0: f64, y0: f64, x1: f64, y1: f64) {
    m.add_polyline(&[p(x0, y0, 0.0), p(x1, y0, 0.0), p(x1, y1, 0.0), p(x0, y1, 0.0)], true, false);
}

fn circle(m: &mut Mesh, c: DVec3, r: f64, n: usize) {
    let pts: Vec<DVec3> = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            c + p(a.cos() * r, a.sin() * r, 0.0)
        })
        .collect();
    m.add_polyline(&pts, true, true);
}

fn check(m: &Mesh) {
    m.validate().unwrap();
}

#[test]
fn four_lines_make_a_face() {
    let mut m = Mesh::new();
    m.add_line(p(0.0, 0.0, 0.0), p(10.0, 0.0, 0.0));
    m.add_line(p(10.0, 0.0, 0.0), p(10.0, 10.0, 0.0));
    m.add_line(p(10.0, 10.0, 0.0), p(0.0, 10.0, 0.0));
    assert_eq!(m.faces.len(), 0);
    m.add_line(p(0.0, 10.0, 0.0), p(0.0, 0.0, 0.0));
    assert_eq!(m.faces.len(), 1);
    assert_eq!(m.edges.len(), 4);
    check(&m);
}

#[test]
fn ground_faces_face_down() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.values().next().unwrap();
    assert!(f.plane.normal.z < -0.99);
    assert!((m.face_area(m.faces.keys().next().unwrap()) - 100.0).abs() < 1e-9);
}

#[test]
fn line_across_face_splits_it() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    m.add_line(p(5.0, 0.0, 0.0), p(5.0, 10.0, 0.0));
    assert_eq!(m.faces.len(), 2);
    assert_eq!(m.edges.len(), 7);
    let total: f64 = m.faces.keys().map(|f| m.face_area(f)).sum();
    assert!((total - 100.0).abs() < 1e-9);
    check(&m);
}

#[test]
fn crossing_lines_split_each_other() {
    let mut m = Mesh::new();
    m.add_line(p(0.0, 0.0, 0.0), p(10.0, 0.0, 0.0));
    m.add_line(p(5.0, -5.0, 0.0), p(5.0, 5.0, 0.0));
    assert_eq!(m.edges.len(), 4);
    assert_eq!(m.vertices.len(), 5);
}

#[test]
fn circle_inside_face_makes_hole_and_disc() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    circle(&mut m, p(5.0, 5.0, 0.0), 2.0, 24);
    assert_eq!(m.faces.len(), 2);
    let with_hole = m.faces.values().filter(|f| f.holes.len() == 1).count();
    assert_eq!(with_hole, 1);
    // Both faces keep the down-facing orientation of the original.
    assert!(m.faces.values().all(|f| f.plane.normal.z < -0.99));
    check(&m);
}

#[test]
fn erasing_splitting_edge_heals() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let e = m.add_line(p(5.0, 0.0, 0.0), p(5.0, 10.0, 0.0));
    assert_eq!(m.faces.len(), 2);
    m.erase_edges(&e);
    assert_eq!(m.faces.len(), 1);
    // Collinear split vertices are merged back.
    assert_eq!(m.edges.len(), 4);
    assert_eq!(m.vertices.len(), 4);
    check(&m);
}

#[test]
fn erasing_boundary_edge_removes_face() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let e = m.edges.keys().next().unwrap();
    m.erase_edges(&[e]);
    assert_eq!(m.faces.len(), 0);
    assert_eq!(m.edges.len(), 3);
    check(&m);
}

#[test]
fn push_pull_rectangle_makes_box() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.keys().next().unwrap();
    // Ground face points down; pulling "up" is a negative distance along its normal.
    m.push_pull(f, -5.0, PushPullMode::Auto);
    assert_eq!(m.faces.len(), 6);
    assert_eq!(m.edges.len(), 12);
    check(&m);
    // All face normals point outward from the box centre.
    let c = p(5.0, 5.0, 2.5);
    for (id, f) in &m.faces {
        let fc = m.face_points(id).iter().fold(DVec3::ZERO, |a, b| a + *b) / f.outer.len() as f64;
        assert!(f.plane.normal.dot(fc - c) > 0.0, "inward face {id:?}");
    }
}

#[test]
fn push_pull_box_top_stretches() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.keys().next().unwrap();
    let top = m.push_pull(f, -5.0, PushPullMode::Auto).unwrap();
    assert!(m.faces[top].plane.normal.z > 0.99);
    m.push_pull(top, 3.0, PushPullMode::Auto);
    assert_eq!(m.faces.len(), 6);
    assert_eq!(m.edges.len(), 12);
    assert!(m.bounds().max.z - 8.0 < 1e-9);
    check(&m);
    // Ctrl push/pull keeps the old top as an interior face and adds 4 sides + a new top.
    m.push_pull(top, 2.0, PushPullMode::NewStartingFace);
    assert_eq!(m.faces.len(), 11);
    check(&m);
}

#[test]
fn push_pull_rectangle_on_wall() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.keys().next().unwrap();
    let top = m.push_pull(f, -10.0, PushPullMode::Auto).unwrap();
    // Draw a smaller rectangle on the top face and pull it up.
    let z = 10.0;
    m.add_polyline(&[p(2.0, 2.0, z), p(4.0, 2.0, z), p(4.0, 4.0, z), p(2.0, 4.0, z)], true, false);
    assert_eq!(m.faces.len(), 7);
    let small = m.faces.keys().find(|f| *f != top && (m.face_area(*f) - 4.0).abs() < 1e-9).unwrap();
    m.push_pull(small, 2.0, PushPullMode::Auto);
    // 5 box faces + top-with-hole + 4 sides + new top.
    assert_eq!(m.faces.len(), 11);
    check(&m);
}

#[test]
fn push_into_solid_carves_outward_facing_recess() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.keys().next().unwrap();
    let top = m.push_pull(f, -10.0, PushPullMode::Auto).unwrap();
    let _ = top;
    let z = 10.0;
    m.add_polyline(&[p(2.0, 2.0, z), p(4.0, 2.0, z), p(4.0, 4.0, z), p(2.0, 4.0, z)], true, false);
    let small = m.faces.keys().find(|f| (m.face_area(*f) - 4.0).abs() < 1e-9).unwrap();
    // Push 3" down into the box.
    let bottom = m.push_pull(small, -3.0, PushPullMode::Auto).unwrap();
    assert!(m.faces[bottom].plane.normal.z > 0.99, "recess floor must face up (out of the solid)");
    let hole_center = p(3.0, 3.0, 8.5);
    for (id, f) in &m.faces {
        let pts = m.face_points(id);
        let c = pts.iter().fold(DVec3::ZERO, |a, b| a + *b) / pts.len() as f64;
        let in_recess_wall = (c.z - 8.5).abs() < 1e-9 && f.plane.normal.z.abs() < 1e-9;
        if in_recess_wall {
            assert!(f.plane.normal.dot(hole_center - c) > 0.0, "recess wall must face into the hole");
        }
    }
    check(&m);
}

#[test]
fn vertical_wall_from_lines() {
    let mut m = Mesh::new();
    m.add_polyline(&[p(0.0, 0.0, 0.0), p(10.0, 0.0, 0.0), p(10.0, 0.0, 8.0), p(0.0, 0.0, 8.0)], true, false);
    assert_eq!(m.faces.len(), 1);
    check(&m);
}

#[test]
fn pick_face_with_ray() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let ray = sc_math::Ray::new(p(5.0, 5.0, 10.0), -DVec3::Z);
    let hit = m.pick_face(&ray, 0.0).unwrap();
    assert!((hit.t - 10.0).abs() < 1e-9);
    let miss = sc_math::Ray::new(p(15.0, 5.0, 10.0), -DVec3::Z);
    assert!(m.pick_face(&miss, 0.0).is_none());
}

#[test]
fn triangulation_area_matches() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    circle(&mut m, p(5.0, 5.0, 0.0), 2.0, 24);
    for f in m.faces.keys() {
        let tri_area: f64 = m.triangulate_face(f).iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5).sum();
        assert!((tri_area - m.face_area(f)).abs() < 1e-6, "{tri_area} vs {}", m.face_area(f));
    }
}

#[test]
fn serde_roundtrip() {
    let mut m = Mesh::new();
    rect(&mut m, 0.0, 0.0, 10.0, 10.0);
    let s = serde_json::to_string(&m).unwrap();
    let mut back: Mesh = serde_json::from_str(&s).unwrap();
    back.rebuild_links();
    assert_eq!(back.faces.len(), 1);
    check(&back);
}

fn unit_box(m: &mut Mesh) {
    rect(m, 0.0, 0.0, 10.0, 10.0);
    let f = m.faces.keys().next().unwrap();
    m.push_pull(f, -10.0, PushPullMode::Auto);
}

#[test]
fn extract_whole_box_and_merge_back() {
    let mut m = Mesh::new();
    unit_box(&mut m);
    let edges: Vec<_> = m.edges.keys().collect();
    let faces: Vec<_> = m.faces.keys().collect();
    let g = m.extract(&edges, &faces, &[]);
    assert!(m.edges.is_empty() && m.faces.is_empty() && m.vertices.is_empty());
    assert_eq!((g.faces.len(), g.edges.len()), (6, 12));
    g.validate().unwrap();

    // Explode it back, moved 20" along X.
    let xf = DMat4::from_translation(DVec3::new(20.0, 0.0, 0.0));
    m.merge_mesh(&g, xf);
    assert_eq!((m.faces.len(), m.edges.len()), (6, 12));
    check(&m);
    let c = p(25.0, 5.0, 5.0);
    for (id, f) in &m.faces {
        let pts = m.face_points(id);
        let fc = pts.iter().fold(DVec3::ZERO, |a, b| a + *b) / pts.len() as f64;
        assert!(f.plane.normal.dot(fc - c) > 0.0, "orientation kept");
    }
}

#[test]
fn extract_one_face_keeps_shared_edges() {
    let mut m = Mesh::new();
    unit_box(&mut m);
    let top = m.faces.iter().find(|(_, f)| f.plane.normal.z > 0.99).map(|(id, _)| id).unwrap();
    let g = m.extract(&[], &[top], &[]);
    assert_eq!((g.faces.len(), g.edges.len()), (1, 4));
    assert_eq!((m.faces.len(), m.edges.len()), (5, 12));
    check(&m);
}

#[test]
fn explode_does_not_invent_faces() {
    // An open "U" of three edges plus a closing edge that was never a face stays faceless.
    let mut g = Mesh::new();
    g.add_line(p(0.0, 0.0, 0.0), p(5.0, 0.0, 0.0));
    g.add_line(p(5.0, 0.0, 0.0), p(5.0, 5.0, 0.0));
    g.add_line(p(5.0, 5.0, 0.0), p(0.0, 5.0, 0.0));
    g.add_line(p(0.0, 5.0, 0.0), p(0.0, 0.0, 0.0));
    let f = g.faces.keys().next().unwrap();
    g.erase_faces(&[f]);
    let mut m = Mesh::new();
    m.merge_mesh(&g, DMat4::IDENTITY);
    assert_eq!(m.faces.len(), 0);
    assert_eq!(m.edges.len(), 4);
}

#[test]
fn append_raw_mirrored_keeps_valid_winding() {
    let mut src = Mesh::new();
    unit_box(&mut src);
    let mut m = Mesh::new();
    m.append_raw(&src, &DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0)));
    assert_eq!(m.faces.len(), 6);
    for f in m.faces.keys() {
        let tri_area: f64 = m.triangulate_face(f).iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).dot(m.faces[f].plane.normal) * 0.5).sum();
        assert!(tri_area > 0.0, "triangles wind CCW around the normal");
    }
}

#[test]
fn follow_me_l_shaped_path_makes_mitered_solid() {
    let mut m = Mesh::new();
    // Path along +X then +Y on the ground.
    let path = vec![p(0.0, 0.0, 0.0), p(20.0, 0.0, 0.0), p(20.0, 20.0, 0.0)];
    m.add_polyline(&path, false, false);
    // 2x2 square profile in the YZ plane at the path start.
    m.add_polyline(&[p(0.0, -1.0, 1.0), p(0.0, 1.0, 1.0), p(0.0, 1.0, 3.0), p(0.0, -1.0, 3.0)], true, false);
    let profile = m.faces.iter().find(|(_, f)| f.plane.normal.x.abs() > 0.99).map(|(id, _)| id).unwrap();
    let faces = m.follow_me(profile, &path, false, &[false; 3]);
    // 2 segments × 4 sides + 2 caps.
    assert_eq!(faces.len(), 10);
    m.validate().unwrap();
    // Mitered corner: ring at the bend lies on the 45° plane through (20, 0).
    assert!(m.find_vertex(p(21.0, -1.0, 1.0)).is_some() && m.find_vertex(p(19.0, 1.0, 3.0)).is_some());
    let vol: f64 = faces.iter().flat_map(|f| m.triangulate_face(*f)).map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum();
    assert!((vol - 2.0 * 2.0 * 40.0).abs() < 1e-6, "volume {vol}");
}

#[test]
fn follow_me_closed_circle_lathe() {
    let mut m = Mesh::new();
    let n = 24;
    let path: Vec<DVec3> = (0..n).map(|i| {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        p(10.0 * a.cos(), 10.0 * a.sin(), 0.0)
    }).collect();
    let edges = m.add_polyline(&path, true, true);
    let (chain, closed) = m.edge_chain(&edges).unwrap();
    assert!(closed && chain.len() == n);
    // Square profile standing on the path at angle 0 (in the XZ plane).
    m.add_polyline(&[p(9.0, 0.0, 1.0), p(11.0, 0.0, 1.0), p(11.0, 0.0, 3.0), p(9.0, 0.0, 3.0)], true, false);
    let profile = m.faces.iter().find(|(_, f)| f.plane.normal.y.abs() > 0.99).map(|(id, _)| id).unwrap();
    let pts: Vec<DVec3> = chain.iter().map(|v| m.pos(*v)).collect();
    let start = pts.iter().position(|q| (q.x - 10.0).abs() < 1e-9 && q.y.abs() < 1e-9).unwrap();
    let mut rotated = pts[start..].to_vec();
    rotated.extend_from_slice(&pts[..start]);
    let faces = m.follow_me(profile, &rotated, true, &vec![true; n]);
    assert_eq!(faces.len(), n * 4, "torus-like ring, no caps");
    m.validate().unwrap();
    let vol: f64 = faces.iter().flat_map(|f| m.triangulate_face(*f)).map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum();
    assert!(vol > 0.0);
    // Edges around the ring at path vertices are soft.
    assert!(m.edges.values().filter(|e| e.soft).count() >= n * 4 - 8);
}
