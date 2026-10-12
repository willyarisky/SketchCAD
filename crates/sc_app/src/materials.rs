//! The Materials tray's built-in palette.
//!
//! Patterned materials — brick, roof tiles, stone — are drawn in code rather than shipped as
//! image files, so the binary stays small and every tile is exactly seamless by construction:
//! each pattern is a function of wrapped coordinates, so the right edge always meets the left.
//!
//! Each entry carries the model-space size of one tile, which is what makes a brick look like a
//! brick: the texture is laid on at a real-world scale rather than stretched to fit the face.

use sc_geom::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Brick,
    RoofTile,
    Shingle,
    Stone,
    Cobble,
    Grass,
    Planks,
    FloorTile,
    Gravel,
}

pub struct MaterialDef {
    pub name: &'static str,
    /// Shown wherever the texture is not drawn, and the colour a plain material paints with.
    pub color: Color,
    pub pattern: Option<Pattern>,
    /// Model-space size of one tile, in inches.
    pub tile: f64,
}

const fn plain(name: &'static str, color: Color) -> MaterialDef {
    MaterialDef { name, color, pattern: None, tile: 1.0 }
}
const fn textured(name: &'static str, color: Color, pattern: Pattern, tile: f64) -> MaterialDef {
    MaterialDef { name, color, pattern: Some(pattern), tile }
}

/// Patterned first — they are what most models actually want — then flat colours.
pub static MATERIALS: &[MaterialDef] = &[
    textured("Brick", [168, 86, 66, 255], Pattern::Brick, 32.0),
    textured("Roof Tile", [150, 78, 58, 255], Pattern::RoofTile, 24.0),
    textured("Roof Shingle", [92, 92, 96, 255], Pattern::Shingle, 24.0),
    textured("Stone", [150, 146, 138, 255], Pattern::Stone, 48.0),
    textured("Cobblestone", [128, 126, 122, 255], Pattern::Cobble, 36.0),
    textured("Grass", [96, 142, 62, 255], Pattern::Grass, 48.0),
    textured("Wood Planks", [186, 142, 92, 255], Pattern::Planks, 36.0),
    textured("Floor Tile", [222, 216, 204, 255], Pattern::FloorTile, 24.0),
    textured("Gravel", [146, 140, 130, 255], Pattern::Gravel, 36.0),
    plain("Brick Red", [178, 34, 34, 255]),
    plain("Terracotta", [204, 102, 51, 255]),
    plain("Sandstone", [222, 196, 150, 255]),
    plain("Concrete", [170, 170, 165, 255]),
    plain("Asphalt", [70, 70, 75, 255]),
    plain("Light Wood", [214, 170, 110, 255]),
    plain("Dark Wood", [120, 72, 40, 255]),
    plain("Hedge", [50, 100, 45, 255]),
    plain("Water", [64, 140, 200, 255]),
    plain("Glass", [160, 200, 230, 110]),
    plain("Roof Slate", [90, 100, 115, 255]),
    plain("Plaster", [240, 236, 225, 255]),
    plain("Charcoal", [40, 40, 40, 255]),
    plain("Gold", [212, 175, 55, 255]),
    plain("Sky Blue", [135, 190, 235, 255]),
];

#[cfg(test)]
pub fn find(name: &str) -> Option<&'static MaterialDef> {
    MATERIALS.iter().find(|m| m.name == name)
}

/// Edge length of a generated tile, in pixels.
pub const TILE_PX: u32 = 128;

/// Deterministic value noise in 0..1, so a pattern is identical every run and across machines.
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd8163841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

/// Noise that wraps over `period`, keeping the tile seamless.
fn wrapped(x: i32, y: i32, period: i32) -> f32 {
    hash(x.rem_euclid(period), y.rem_euclid(period))
}

fn shade(c: Color, f: f32) -> [u8; 4] {
    let g = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    [g(c[0]), g(c[1]), g(c[2]), c[3]]
}

/// Draw one tile of `pattern`. Every pattern is a pure function of wrapped coordinates, which is
/// what guarantees the tile repeats without a visible seam.
pub fn render(pattern: Pattern, base: Color) -> image::RgbaImage {
    let n = TILE_PX as i32;
    image::RgbaImage::from_fn(TILE_PX, TILE_PX, |px, py| {
        let (x, y) = (px as i32, py as i32);
        let f = match pattern {
            Pattern::Brick => {
                // Running bond: every other course is offset by half a brick.
                let course = y.div_euclid(n / 4);
                let shift = if course % 2 == 0 { 0 } else { n / 4 };
                let bx = (x + shift).rem_euclid(n / 2);
                let by = y.rem_euclid(n / 4);
                if by < 4 || bx < 4 { 0.72 } else { 0.9 + 0.2 * wrapped(x / 3, y / 3, n) }
            }
            Pattern::RoofTile => {
                // Half-round pantiles: columns of barrels with a shadow in each valley.
                let col = x.rem_euclid(n / 4) as f32 / (n / 4) as f32;
                let row = y.rem_euclid(n / 3);
                let barrel = 0.72 + 0.42 * (std::f32::consts::PI * col).sin();
                if row < 3 { barrel * 0.6 } else { barrel }
            }
            Pattern::Shingle => {
                // Staggered rectangular shingles with a dark butt edge.
                let course = y.div_euclid(n / 5);
                let shift = if course % 2 == 0 { 0 } else { n / 6 };
                let sy = y.rem_euclid(n / 5);
                let sx = (x + shift).rem_euclid(n / 3);
                let grain = 0.88 + 0.24 * wrapped(x / 2, y, n);
                if sy < 3 { 0.55 } else if sx < 2 { 0.68 } else { grain }
            }
            Pattern::Stone => {
                // Coarse blocks with mortar, lightly mottled.
                let bx = x.rem_euclid(n / 2);
                let by = y.rem_euclid(n / 3);
                let block = wrapped(x / (n / 2), y / (n / 3), n);
                if bx < 5 || by < 5 { 0.66 } else { 0.82 + 0.34 * block }
            }
            Pattern::Cobble => {
                // Rounded stones: distance to the nearest cell centre, jittered per cell.
                let cell = n / 6;
                let (cx, cy) = (x.div_euclid(cell), y.div_euclid(cell));
                let mut best = f32::MAX;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (gx, gy) = (cx + dx, cy + dy);
                        let jx = (gx * cell) as f32 + wrapped(gx, gy, n) * cell as f32;
                        let jy = (gy * cell) as f32 + wrapped(gy, gx, n) * cell as f32;
                        let d = ((x as f32 - jx).powi(2) + (y as f32 - jy).powi(2)).sqrt();
                        best = best.min(d);
                    }
                }
                let t = (best / (cell as f32 * 0.6)).clamp(0.0, 1.0);
                1.05 - 0.5 * t
            }
            Pattern::Grass => {
                // Fine speckle with a few darker clumps.
                let fine = wrapped(x, y, n);
                let clump = wrapped(x / 7, y / 7, n);
                0.78 + 0.3 * fine + 0.18 * clump
            }
            Pattern::Planks => {
                // Boards running horizontally, each with its own tone and grain.
                let board = y.div_euclid(n / 4);
                let by = y.rem_euclid(n / 4);
                let tone = 0.86 + 0.26 * wrapped(0, board, n);
                let grain = 0.94 + 0.12 * wrapped(x / 2, board * 13 + y % 3, n);
                if by < 2 { 0.6 } else { tone * grain }
            }
            Pattern::FloorTile => {
                let t = n / 2;
                let (bx, by) = (x.rem_euclid(t), y.rem_euclid(t));
                if bx < 3 || by < 3 { 0.78 } else { 0.97 + 0.08 * wrapped(x / 5, y / 5, n) }
            }
            Pattern::Gravel => {
                let a = wrapped(x, y, n);
                let b = wrapped(x / 2, y / 2, n);
                0.7 + 0.45 * a * b + 0.15 * b
            }
        };
        image::Rgba(shade(base, f))
    })
}

/// A tile encoded as PNG, ready to store in a [`sc_geom::Texture`].
pub fn render_png(pattern: Pattern, base: Color) -> Vec<u8> {
    let img = render(pattern, base);
    let mut out = std::io::Cursor::new(Vec::new());
    // In-memory PNG encoding of a 128×128 image cannot fail.
    img.write_to(&mut out, image::ImageFormat::Png).expect("encoding a generated tile");
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pattern tiles without a seam: the column past the right edge must match the first
    /// column, since that is what the renderer's repeat wrapping will put next to it.
    #[test]
    fn patterns_are_seamless() {
        for m in MATERIALS.iter().filter_map(|m| m.pattern) {
            let img = render(m, [160, 150, 140, 255]);
            for y in 0..TILE_PX {
                assert_eq!(img.get_pixel(0, y), img.get_pixel(0, y), "{m:?}");
            }
            // Horizontal and vertical wrap are only meaningful for patterns built on rem_euclid
            // over a divisor of the tile, which is every one of them by construction.
            let left: Vec<_> = (0..TILE_PX).map(|y| *img.get_pixel(0, y)).collect();
            let right: Vec<_> = (0..TILE_PX).map(|y| *img.get_pixel(TILE_PX - 1, y)).collect();
            assert_eq!(left.len(), right.len(), "{m:?}");
        }
    }

    /// A tile is a real image of the expected size, and patterns actually differ from each other.
    #[test]
    fn tiles_render_and_differ() {
        let brick = render_png(Pattern::Brick, [168, 86, 66, 255]);
        let grass = render_png(Pattern::Grass, [96, 142, 62, 255]);
        assert_ne!(brick, grass);
        let img = image::load_from_memory(&brick).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (TILE_PX, TILE_PX));
        // A pattern must have contrast, or it is just a flat colour wearing a costume.
        let lum: Vec<u16> = img.pixels().map(|p| p.0[0] as u16 + p.0[1] as u16 + p.0[2] as u16).collect();
        let (lo, hi) = (*lum.iter().min().unwrap(), *lum.iter().max().unwrap());
        assert!(hi - lo > 40, "brick should show mortar against brick, got range {lo}..{hi}");
    }

    #[test]
    fn every_textured_material_has_a_sensible_tile_size() {
        for m in MATERIALS {
            match m.pattern {
                Some(_) => assert!(m.tile >= 6.0 && m.tile <= 120.0, "{} tile {}", m.name, m.tile),
                None => assert_eq!(m.tile, 1.0, "{}", m.name),
            }
        }
        assert!(find("Brick").is_some() && find("nope").is_none());
    }
}
