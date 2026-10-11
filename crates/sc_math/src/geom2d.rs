//! 2D polygon helpers used for planar face tracing and triangulation.

use glam::DVec2;

/// Signed area (positive for counter-clockwise).
pub fn signed_area(poly: &[DVec2]) -> f64 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Even-odd point-in-polygon test. Points on the boundary may go either way.
pub fn point_in_polygon(p: DVec2, poly: &[DVec2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Distance from `p` to the boundary of `poly`.
pub fn distance_to_boundary(p: DVec2, poly: &[DVec2]) -> f64 {
    let mut best = f64::INFINITY;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let ab = b - a;
        let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min(p.distance(a + ab * t));
    }
    best
}

/// A point strictly inside a simple polygon (with optional holes), robust for concave shapes.
///
/// Scans horizontal lines through the polygon and picks the midpoint of the widest interior span.
pub fn interior_point(outer: &[DVec2], holes: &[Vec<DVec2>]) -> Option<DVec2> {
    let (mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in outer {
        miny = miny.min(p.y);
        maxy = maxy.max(p.y);
    }
    if !maxy.is_finite() || maxy <= miny {
        return None;
    }
    let mut best: Option<(f64, DVec2)> = None;
    const SAMPLES: usize = 17;
    for k in 1..SAMPLES {
        let y = miny + (maxy - miny) * (k as f64 / SAMPLES as f64);
        let mut xs = Vec::new();
        let mut collect = |poly: &[DVec2]| {
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
            }
        };
        collect(outer);
        for h in holes {
            collect(h);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in xs.chunks(2) {
            if let [x0, x1] = pair {
                let w = x1 - x0;
                if best.is_none_or(|(bw, _)| w > bw) {
                    best = Some((w, DVec2::new((x0 + x1) * 0.5, y)));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Triangulate a polygon with holes using ear clipping after bridging holes into the outer ring.
/// Returns indices into the concatenation `outer ++ holes[0] ++ holes[1] ...`.
pub fn triangulate(outer: &[DVec2], holes: &[Vec<DVec2>]) -> Vec<[u32; 3]> {
    // Build index ring for outer (CCW) and bridge holes (CW) one by one.
    let mut pts: Vec<DVec2> = outer.to_vec();
    let mut ring: Vec<u32> = (0..outer.len() as u32).collect();
    if signed_area(outer) < 0.0 {
        ring.reverse();
    }
    // Sort holes by max x descending (standard bridging order).
    let mut hole_refs: Vec<(usize, &Vec<DVec2>)> = holes.iter().enumerate().collect();
    let mut offsets = Vec::with_capacity(holes.len());
    let mut off = outer.len();
    for h in holes {
        offsets.push(off);
        off += h.len();
        pts.extend_from_slice(h);
    }
    hole_refs.sort_by(|a, b| {
        let ma = a.1.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let mb = b.1.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        mb.partial_cmp(&ma).unwrap()
    });
    for (hi, h) in hole_refs {
        if h.len() < 3 {
            continue;
        }
        let base = offsets[hi] as u32;
        let mut hring: Vec<u32> = (0..h.len() as u32).map(|i| base + i).collect();
        if signed_area(h) > 0.0 {
            hring.reverse();
        }
        // Rightmost hole vertex.
        let (mi, _) = hring
            .iter()
            .enumerate()
            .max_by(|a, b| pts[*a.1 as usize].x.partial_cmp(&pts[*b.1 as usize].x).unwrap())
            .unwrap();
        let hp = pts[hring[mi] as usize];
        // Choose the closest visible ring vertex (no ring edge crosses the bridge).
        let mut best: Option<(f64, usize)> = None;
        for (ri, &rv) in ring.iter().enumerate() {
            let rp = pts[rv as usize];
            let d = rp.distance_squared(hp);
            if best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            if bridge_visible(hp, rp, &pts, &ring, &hring) {
                best = Some((d, ri));
            }
        }
        let Some((_, ri)) = best else { continue };
        let mut new_ring = Vec::with_capacity(ring.len() + hring.len() + 2);
        new_ring.extend_from_slice(&ring[..=ri]);
        for k in 0..=hring.len() {
            new_ring.push(hring[(mi + k) % hring.len()]);
        }
        new_ring.push(ring[ri]);
        new_ring.extend_from_slice(&ring[ri + 1..]);
        ring = new_ring;
    }
    ear_clip(&pts, ring)
}

fn segments_properly_cross(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let o = |p: DVec2, q: DVec2, r: DVec2| (q - p).perp_dot(r - p);
    let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    let eps = 1e-12;
    ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps)) && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
}

fn bridge_visible(a: DVec2, b: DVec2, pts: &[DVec2], ring: &[u32], hring: &[u32]) -> bool {
    for r in [ring, hring] {
        for i in 0..r.len() {
            let p = pts[r[i] as usize];
            let q = pts[r[(i + 1) % r.len()] as usize];
            if segments_properly_cross(a, b, p, q) {
                return false;
            }
        }
    }
    true
}

fn ear_clip(pts: &[DVec2], mut ring: Vec<u32>) -> Vec<[u32; 3]> {
    let mut tris = Vec::new();
    let cross = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a);
    let mut guard = 0usize;
    while ring.len() > 3 {
        let n = ring.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let (a, b, c) = (pts[ia as usize], pts[ib as usize], pts[ic as usize]);
            let cr = cross(a, b, c);
            if cr <= 1e-14 {
                // Reflex or degenerate. Drop exact degenerates (collinear duplicates from bridges).
                if cr.abs() <= 1e-14 && (a.distance_squared(b) < 1e-20 || b.distance_squared(c) < 1e-20) {
                    ring.remove(i);
                    clipped = true;
                    break;
                }
                continue;
            }
            let mut contains = false;
            for &k in ring.iter() {
                if k == ia || k == ib || k == ic {
                    continue;
                }
                let p = pts[k as usize];
                if p == a || p == b || p == c {
                    continue;
                }
                if cross(a, b, p) >= -1e-14 && cross(b, c, p) >= -1e-14 && cross(c, a, p) >= -1e-14 {
                    contains = true;
                    break;
                }
            }
            if contains {
                continue;
            }
            tris.push([ia, ib, ic]);
            ring.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Numerical trouble: fall back to a fan so the face is still visible.
            guard += 1;
            if guard > 2 {
                for i in 1..ring.len() - 1 {
                    tris.push([ring[0], ring[i], ring[i + 1]]);
                }
                return tris;
            }
            ring.rotate_left(1);
        }
    }
    if ring.len() == 3 {
        tris.push([ring[0], ring[1], ring[2]]);
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x0: f64, y0: f64, s: f64) -> Vec<DVec2> {
        vec![DVec2::new(x0, y0), DVec2::new(x0 + s, y0), DVec2::new(x0 + s, y0 + s), DVec2::new(x0, y0 + s)]
    }

    fn tri_area(pts: &[DVec2], t: &[u32; 3]) -> f64 {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        (b - a).perp_dot(c - a).abs() * 0.5
    }

    #[test]
    fn square_area() {
        assert_eq!(signed_area(&sq(0.0, 0.0, 2.0)), 4.0);
    }

    #[test]
    fn triangulate_square_with_hole() {
        let outer = sq(0.0, 0.0, 10.0);
        let hole = sq(3.0, 3.0, 4.0);
        let tris = triangulate(&outer, std::slice::from_ref(&hole));
        let mut all = outer.clone();
        all.extend(hole);
        let area: f64 = tris.iter().map(|t| tri_area(&all, t)).sum();
        assert!((area - 84.0).abs() < 1e-9, "area {area}");
    }

    #[test]
    fn triangulate_concave_l() {
        let l = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(2.0, 0.0),
            DVec2::new(2.0, 1.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(1.0, 2.0),
            DVec2::new(0.0, 2.0),
        ];
        let tris = triangulate(&l, &[]);
        assert_eq!(tris.len(), 4);
        let area: f64 = tris.iter().map(|t| tri_area(&l, t)).sum();
        assert!((area - 3.0).abs() < 1e-9);
    }

    #[test]
    fn interior_point_of_l() {
        let l = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(2.0, 0.0),
            DVec2::new(2.0, 1.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(1.0, 2.0),
            DVec2::new(0.0, 2.0),
        ];
        let p = interior_point(&l, &[]).unwrap();
        assert!(point_in_polygon(p, &l));
    }
}
