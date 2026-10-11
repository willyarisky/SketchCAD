//! SketchCAD's own vector icon set, drawn with egui shapes on a 24×24 grid.
//!
//! The icons follow SketchUp's visual language (yellow pencils, red/green/blue axis colors,
//! isometric boxes) so the toolbars feel familiar, but every drawing here is original.

use crate::tools::ToolId;
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Tool(ToolId),
    ZoomExtents,
    Undo,
    Redo,
    New,
    Open,
    Save,
    ViewIso,
    ViewTop,
    ViewFront,
    ViewRight,
    ViewBack,
    ViewLeft,
    XRay,
    BackEdges,
    Wireframe,
    HiddenLine,
    Shaded,
    Monochrome,
}

const INK: Color32 = Color32::from_rgb(45, 45, 50);
const PENCIL: Color32 = Color32::from_rgb(242, 190, 40);
const PENCIL_DARK: Color32 = Color32::from_rgb(200, 140, 20);
const RED: Color32 = Color32::from_rgb(214, 48, 39);
const BLUE: Color32 = Color32::from_rgb(40, 95, 190);
const LIGHT_BLUE: Color32 = Color32::from_rgb(175, 205, 240);
const BOX_TOP: Color32 = Color32::from_rgb(250, 225, 120);
const BOX_LEFT: Color32 = Color32::from_rgb(225, 225, 230);
const BOX_RIGHT: Color32 = Color32::from_rgb(170, 175, 185);

struct G {
    r: Rect,
}

impl G {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.r.left() + x / 24.0 * self.r.width(), self.r.top() + y / 24.0 * self.r.height())
    }
    fn s(&self, w: f32) -> f32 {
        w * self.r.width() / 24.0
    }
    fn pts(&self, v: &[(f32, f32)]) -> Vec<Pos2> {
        v.iter().map(|(x, y)| self.p(*x, *y)).collect()
    }
    fn poly(&self, pa: &Painter, v: &[(f32, f32)], fill: Color32, stroke: Color32) {
        pa.add(Shape::convex_polygon(self.pts(v), fill, Stroke::new(self.s(1.0), stroke)));
    }
    fn line(&self, pa: &Painter, v: &[(f32, f32)], w: f32, c: Color32) {
        pa.add(Shape::line(self.pts(v), Stroke::new(self.s(w), c)));
    }
    fn circle(&self, pa: &Painter, c: (f32, f32), r: f32, fill: Color32, stroke: Stroke) {
        pa.circle(self.p(c.0, c.1), self.s(r), fill, Stroke::new(self.s(stroke.width), stroke.color));
    }
    fn arc(&self, c: (f32, f32), r: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
        let n = 20;
        (0..=n).map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        }).collect()
    }
    fn arrow_head(&self, pa: &Painter, tip: (f32, f32), dir: (f32, f32), size: f32, c: Color32) {
        let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-3);
        let (dx, dy) = (dir.0 / len, dir.1 / len);
        let (nx, ny) = (-dy, dx);
        let base = (tip.0 - dx * size, tip.1 - dy * size);
        self.poly(pa, &[tip, (base.0 + nx * size * 0.6, base.1 + ny * size * 0.6), (base.0 - nx * size * 0.6, base.1 - ny * size * 0.6)], c, c);
    }

    /// Small pencil in the lower right, used as a "drawing tool" badge.
    fn pencil(&self, pa: &Painter) {
        self.poly(pa, &[(13.0, 21.0), (12.0, 23.5), (14.5, 22.5)], Color32::from_rgb(250, 220, 180), INK);
        self.poly(pa, &[(13.0, 21.0), (21.0, 13.0), (22.5, 14.5), (14.5, 22.5)], PENCIL, PENCIL_DARK);
        self.poly(pa, &[(21.0, 13.0), (22.0, 12.0), (23.5, 13.5), (22.5, 14.5)], RED, RED);
    }

    /// Isometric box. `top`, `left`, `right` face fills.
    fn iso_box(&self, pa: &Painter, top: Color32, left: Color32, right: Color32) {
        let (t, l, r) = ([(12.0, 4.0), (21.0, 8.5), (12.0, 13.0), (3.0, 8.5)], [(3.0, 8.5), (12.0, 13.0), (12.0, 22.0), (3.0, 17.5)], [(12.0, 13.0), (21.0, 8.5), (21.0, 17.5), (12.0, 22.0)]);
        self.poly(pa, &l, left, INK);
        self.poly(pa, &r, right, INK);
        self.poly(pa, &t, top, INK);
    }
}

pub fn paint(pa: &Painter, rect: Rect, icon: Icon, enabled: bool) {
    let g = G { r: rect };
    let mut faded = pa.clone();
    if !enabled {
        faded.multiply_opacity(0.35);
    }
    let pa = &faded;
    match icon {
        Icon::Tool(t) => paint_tool(pa, &g, t),
        Icon::ZoomExtents => {
            magnifier(pa, &g);
            for (x, y, dx, dy) in [(2.0, 2.0, 1.0, 1.0), (22.0, 2.0, -1.0, 1.0), (2.0, 22.0, 1.0, -1.0), (22.0, 22.0, -1.0, -1.0)] {
                g.line(pa, &[(x + dx * 5.0, y), (x, y), (x, y + dy * 5.0)], 1.6, RED);
            }
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let pts: Vec<(f32, f32)> = g.arc((12.0, 13.0), 7.0, std::f32::consts::PI * 1.1, std::f32::consts::PI * 2.1).into_iter().map(|(x, y)| (12.0 + (x - 12.0) * flip, y)).collect();
            g.line(pa, &pts, 2.2, BLUE);
            let tip = pts[0];
            g.arrow_head(pa, (tip.0, tip.1 + 2.0), (0.0, 1.0), 4.0, BLUE);
        }
        Icon::New => {
            g.poly(pa, &[(5.0, 2.0), (15.0, 2.0), (19.0, 6.0), (19.0, 22.0), (5.0, 22.0)], Color32::WHITE, INK);
            g.line(pa, &[(15.0, 2.0), (15.0, 6.0), (19.0, 6.0)], 1.0, INK);
        }
        Icon::Open => {
            g.poly(pa, &[(2.0, 6.0), (9.0, 6.0), (11.0, 8.0), (20.0, 8.0), (20.0, 20.0), (2.0, 20.0)], PENCIL, PENCIL_DARK);
            g.poly(pa, &[(5.0, 11.0), (23.0, 11.0), (20.0, 20.0), (2.0, 20.0)], BOX_TOP, PENCIL_DARK);
        }
        Icon::Save => {
            g.poly(pa, &[(3.0, 3.0), (18.0, 3.0), (21.0, 6.0), (21.0, 21.0), (3.0, 21.0)], BLUE, INK);
            g.poly(pa, &[(7.0, 3.0), (16.0, 3.0), (16.0, 9.0), (7.0, 9.0)], Color32::WHITE, INK);
            g.poly(pa, &[(6.0, 13.0), (18.0, 13.0), (18.0, 21.0), (6.0, 21.0)], Color32::from_gray(235), INK);
        }
        Icon::ViewIso => g.iso_box(pa, BOX_TOP, BOX_LEFT, BOX_RIGHT),
        Icon::ViewTop => g.iso_box(pa, LIGHT_BLUE, BOX_LEFT, BOX_RIGHT),
        Icon::ViewFront => g.iso_box(pa, BOX_LEFT, LIGHT_BLUE, BOX_RIGHT),
        Icon::ViewRight => g.iso_box(pa, BOX_LEFT, BOX_LEFT, LIGHT_BLUE),
        Icon::ViewBack | Icon::ViewLeft => {
            g.iso_box(pa, BOX_LEFT, BOX_LEFT, BOX_RIGHT);
            let side = if icon == Icon::ViewBack { [(12.0, 4.0), (21.0, 8.5), (21.0, 17.5), (12.0, 13.0)] } else { [(3.0, 8.5), (12.0, 4.0), (12.0, 13.0), (3.0, 17.5)] };
            pa.add(Shape::dashed_line(&g.pts(&[side[0], side[1], side[2], side[3], side[0]]), Stroke::new(g.s(1.2), BLUE), g.s(2.0), g.s(1.5)));
        }
        Icon::XRay => {
            let c = Color32::from_rgba_unmultiplied(160, 180, 210, 90);
            g.iso_box(pa, c, c, c);
            pa.add(Shape::dashed_line(&g.pts(&[(3.0, 17.5), (12.0, 13.0), (21.0, 17.5)]), Stroke::new(g.s(1.0), INK), g.s(1.5), g.s(1.5)));
        }
        Icon::BackEdges => {
            g.iso_box(pa, Color32::WHITE, Color32::WHITE, Color32::WHITE);
            pa.add(Shape::dashed_line(&g.pts(&[(3.0, 17.5), (12.0, 13.0), (21.0, 17.5)]), Stroke::new(g.s(1.0), RED), g.s(1.5), g.s(1.5)));
        }
        Icon::Wireframe => {
            let t = Color32::TRANSPARENT;
            g.iso_box(pa, t, t, t);
            g.line(pa, &[(3.0, 17.5), (12.0, 13.0), (21.0, 17.5)], 1.0, INK);
            g.line(pa, &[(12.0, 4.0), (12.0, 13.0)], 1.0, INK);
        }
        Icon::HiddenLine => g.iso_box(pa, Color32::WHITE, Color32::WHITE, Color32::WHITE),
        Icon::Shaded => g.iso_box(pa, Color32::from_rgb(245, 245, 245), Color32::from_rgb(205, 210, 220), Color32::from_rgb(150, 160, 175)),
        Icon::Monochrome => g.iso_box(pa, Color32::WHITE, Color32::from_gray(200), Color32::from_gray(150)),
    }
}

fn magnifier(pa: &Painter, g: &G) {
    g.circle(pa, (10.0, 10.0), 6.0, Color32::from_rgba_unmultiplied(200, 225, 250, 180), Stroke::new(2.0, INK));
    g.line(pa, &[(14.5, 14.5), (21.0, 21.0)], 3.2, INK);
}

fn paint_tool(pa: &Painter, g: &G, t: ToolId) {
    match t {
        ToolId::Select => {
            g.poly(pa, &[(7.0, 3.0), (7.0, 19.0), (10.8, 15.4), (17.5, 15.0)], Color32::WHITE, INK);
            g.poly(pa, &[(10.0, 15.0), (12.0, 14.0), (15.0, 20.5), (13.0, 21.5)], Color32::WHITE, INK);
            g.line(pa, &[(7.0, 3.0), (7.0, 19.0), (10.8, 15.4), (17.5, 15.0), (7.0, 3.0)], 1.3, INK);
        }
        ToolId::Eraser => {
            g.poly(pa, &[(3.0, 15.0), (12.0, 6.0), (18.0, 12.0), (9.0, 21.0)], Color32::from_rgb(240, 150, 180), INK);
            g.poly(pa, &[(12.0, 6.0), (15.0, 3.0), (21.0, 9.0), (18.0, 12.0)], Color32::WHITE, INK);
            g.line(pa, &[(9.0, 21.0), (20.0, 21.0)], 1.2, INK);
        }
        ToolId::PaintBucket => {
            g.poly(pa, &[(4.0, 10.0), (11.0, 4.0), (19.0, 12.0), (12.0, 19.0)], Color32::from_gray(210), INK);
            g.line(pa, &[(11.0, 4.0), (8.0, 2.0), (5.0, 5.0)], 1.2, INK);
            g.poly(pa, &[(19.0, 12.0), (21.0, 16.0), (22.0, 20.0), (20.0, 21.0), (18.5, 18.0), (17.5, 14.0)], BLUE, BLUE);
            g.poly(pa, &[(4.0, 10.0), (19.0, 12.0), (12.0, 19.0)], Color32::from_rgb(90, 140, 220), INK);
        }
        ToolId::Line => {
            g.line(pa, &[(2.0, 22.0), (8.0, 16.0)], 1.5, INK);
            g.poly(pa, &[(8.0, 16.0), (6.5, 19.5), (10.0, 18.0)], Color32::from_rgb(250, 220, 180), INK);
            g.poly(pa, &[(8.0, 16.0), (19.0, 5.0), (21.0, 7.0), (10.0, 18.0)], PENCIL, PENCIL_DARK);
            g.poly(pa, &[(19.0, 5.0), (20.5, 3.5), (22.5, 5.5), (21.0, 7.0)], RED, RED);
        }
        ToolId::Arc => {
            let pts = g.arc((12.0, 18.0), 9.0, std::f32::consts::PI, std::f32::consts::PI * 1.75);
            g.line(pa, &pts, 1.8, RED);
            g.circle(pa, (3.0, 18.0), 1.4, INK, Stroke::NONE);
            g.pencil(pa);
        }
        ToolId::Rectangle => {
            g.poly(pa, &[(2.0, 4.0), (18.0, 4.0), (18.0, 16.0), (2.0, 16.0)], LIGHT_BLUE, BLUE);
            g.pencil(pa);
        }
        ToolId::Circle => {
            g.circle(pa, (10.0, 10.0), 7.5, LIGHT_BLUE, Stroke::new(1.3, BLUE));
            g.circle(pa, (10.0, 10.0), 0.9, BLUE, Stroke::NONE);
            g.pencil(pa);
        }
        ToolId::Polygon => {
            let hex: Vec<(f32, f32)> = (0..6).map(|i| {
                let a = std::f32::consts::PI / 3.0 * i as f32;
                (10.0 + 8.0 * a.cos(), 10.0 + 8.0 * a.sin())
            }).collect();
            g.poly(pa, &hex, LIGHT_BLUE, BLUE);
            g.pencil(pa);
        }
        ToolId::PushPull => {
            g.poly(pa, &[(3.0, 13.0), (12.0, 17.5), (12.0, 22.5), (3.0, 18.0)], BOX_LEFT, INK);
            g.poly(pa, &[(12.0, 17.5), (21.0, 13.0), (21.0, 18.0), (12.0, 22.5)], BOX_RIGHT, INK);
            g.poly(pa, &[(12.0, 8.5), (21.0, 13.0), (12.0, 17.5), (3.0, 13.0)], BOX_TOP, INK);
            g.line(pa, &[(12.0, 13.0), (12.0, 4.0)], 2.4, RED);
            g.arrow_head(pa, (12.0, 1.0), (0.0, -1.0), 4.0, RED);
        }
        ToolId::Move => {
            g.line(pa, &[(12.0, 4.0), (12.0, 20.0)], 2.0, RED);
            g.line(pa, &[(4.0, 12.0), (20.0, 12.0)], 2.0, RED);
            for (tip, dir) in [((12.0, 1.5), (0.0, -1.0)), ((12.0, 22.5), (0.0, 1.0)), ((1.5, 12.0), (-1.0, 0.0)), ((22.5, 12.0), (1.0, 0.0))] {
                g.arrow_head(pa, tip, dir, 4.0, RED);
            }
        }
        ToolId::Rotate => {
            let half = g.arc((12.0, 15.0), 9.0, std::f32::consts::PI, std::f32::consts::TAU);
            let mut fill = half.clone();
            fill.push((12.0, 15.0));
            pa.add(Shape::convex_polygon(g.pts(&fill), LIGHT_BLUE, Stroke::new(g.s(1.0), BLUE)));
            for i in 1..6 {
                let a = std::f32::consts::PI * (1.0 + i as f32 / 6.0);
                g.line(pa, &[(12.0 + 9.0 * a.cos(), 15.0 + 9.0 * a.sin()), (12.0 + 7.0 * a.cos(), 15.0 + 7.0 * a.sin())], 0.8, BLUE);
            }
            let arc = g.arc((12.0, 15.0), 5.0, std::f32::consts::PI * 1.15, std::f32::consts::PI * 1.75);
            g.line(pa, &arc, 1.8, RED);
            let tip = *arc.last().unwrap();
            g.arrow_head(pa, (tip.0 + 1.0, tip.1 + 1.2), (1.0, 1.4), 3.5, RED);
            g.line(pa, &[(2.0, 15.0), (22.0, 15.0)], 1.2, INK);
        }
        ToolId::Scale => {
            g.poly(pa, &[(3.0, 9.0), (15.0, 9.0), (15.0, 21.0), (3.0, 21.0)], LIGHT_BLUE, BLUE);
            g.line(pa, &[(3.0, 9.0), (3.0, 3.0), (21.0, 3.0), (21.0, 21.0), (15.0, 21.0)], 1.2, INK);
            g.line(pa, &[(13.0, 11.0), (19.0, 5.0)], 1.8, RED);
            g.arrow_head(pa, (20.5, 3.5), (1.0, -1.0), 4.0, RED);
            for (x, y) in [(3.0, 3.0), (21.0, 3.0), (21.0, 21.0), (3.0, 21.0), (15.0, 9.0)] {
                g.poly(pa, &[(x - 1.5, y - 1.5), (x + 1.5, y - 1.5), (x + 1.5, y + 1.5), (x - 1.5, y + 1.5)], Color32::from_rgb(40, 170, 40), INK);
            }
        }
        ToolId::FollowMe => {
            // A profile swept along a curved path.
            let path = g.arc((22.0, 22.0), 16.0, std::f32::consts::PI, std::f32::consts::PI * 1.5);
            g.line(pa, &path, 1.6, RED);
            g.poly(pa, &[(3.0, 15.0), (9.0, 15.0), (9.0, 23.0), (3.0, 23.0)], BOX_TOP, INK);
            g.poly(pa, &[(15.0, 3.0), (23.0, 3.0), (23.0, 9.0), (15.0, 9.0)], BOX_TOP, INK);
            g.arrow_head(pa, (13.0, 7.5), (1.0, -0.4), 3.5, RED);
        }
        ToolId::Offset => {
            g.poly(pa, &[(2.0, 3.0), (22.0, 3.0), (22.0, 21.0), (2.0, 21.0)], LIGHT_BLUE, BLUE);
            g.line(pa, &[(7.0, 8.0), (17.0, 8.0), (17.0, 16.0), (7.0, 16.0), (7.0, 8.0)], 1.6, RED);
            g.arrow_head(pa, (7.5, 8.5), (-1.0, -1.0), 3.0, RED);
        }
        ToolId::TapeMeasure => {
            g.poly(pa, &[(10.0, 15.0), (22.0, 15.0), (22.0, 19.0), (10.0, 19.0)], PENCIL, PENCIL_DARK);
            for x in [13.0, 16.0, 19.0] {
                g.line(pa, &[(x, 15.0), (x, 17.0)], 0.8, INK);
            }
            g.circle(pa, (9.0, 11.0), 7.5, PENCIL, Stroke::new(1.2, PENCIL_DARK));
            g.circle(pa, (9.0, 11.0), 2.5, Color32::from_gray(80), Stroke::NONE);
        }
        ToolId::Orbit => {
            let a = g.arc((12.0, 12.0), 8.0, std::f32::consts::PI * 0.9, std::f32::consts::PI * 1.9);
            g.line(pa, &a, 2.0, RED);
            g.arrow_head(pa, (19.5, 9.0), (0.3, 1.0), 4.0, RED);
            let b = g.arc((12.0, 12.0), 8.0, -std::f32::consts::PI * 0.1, std::f32::consts::PI * 0.9);
            g.line(pa, &b, 2.0, BLUE);
            g.arrow_head(pa, (4.5, 15.0), (-0.3, -1.0), 4.0, BLUE);
            g.circle(pa, (12.0, 12.0), 2.0, INK, Stroke::NONE);
        }
        ToolId::Pan => {
            let skin = Color32::from_rgb(250, 235, 215);
            for (x, top) in [(6.5, 6.0), (9.5, 3.5), (12.5, 3.0), (15.5, 4.5)] {
                g.poly(pa, &[(x - 1.4, top), (x + 1.4, top), (x + 1.4, 13.0), (x - 1.4, 13.0)], skin, INK);
            }
            g.poly(pa, &[(5.0, 11.0), (17.0, 11.0), (17.0, 18.0), (14.0, 21.5), (8.0, 21.5), (5.0, 17.0)], skin, INK);
            g.poly(pa, &[(17.0, 13.0), (20.5, 10.0), (21.5, 11.5), (17.0, 17.0)], skin, INK);
        }
        ToolId::Zoom => magnifier(pa, g),
    }
}

/// Paint an icon button; returns the click response.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, enabled: bool, size: f32, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    if ui.is_rect_visible(rect) {
        let vis = ui.visuals();
        if selected {
            ui.painter().rect(rect, 3.0, vis.selection.bg_fill.gamma_multiply(0.45), Stroke::new(1.0, vis.selection.stroke.color), StrokeKind::Inside);
        } else if resp.hovered() && enabled {
            ui.painter().rect(rect, 3.0, vis.widgets.hovered.weak_bg_fill, Stroke::new(1.0, vis.widgets.hovered.bg_stroke.color), StrokeKind::Inside);
        }
        paint(ui.painter(), rect.shrink(size * 0.12), icon, enabled);
    }
    resp.on_hover_text(tooltip)
}
