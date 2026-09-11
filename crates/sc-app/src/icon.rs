//! Line icons, drawn rather than shipped.
//!
//! Laid out on the same 24 by 24 grid with the same 2px stroke that Lucide uses,
//! so they sit together as a set. Drawing them avoids an icon font, keeps them
//! crisp at any interface scale, and lets them take the current text colour.

use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2};

/// Which icon to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Pen,
    Square,
    Circle,
    Hexagon,
    Cube,
    Sphere,
    Cylinder,
    Slot,
    Hole,
    HexHole,
    Shell,
    Offset,
    Move,
    Undo,
    Redo,
    File,
    Folder,
    Save,
    Download,
    Frame,
    Cursor,
    Help,
    Sun,
    Moon,
    Monitor,
    Layers,
    Plane,
    Union,
    Subtract,
    Intersect,
    Torus,
    Extrude,
    Trash,
}

/// Draws `icon` centred in `rect`.
pub(crate) fn draw(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    // Keep the grid square so nothing is stretched.
    let side = rect.width().min(rect.height());
    let square = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let pen = Pen {
        painter,
        rect: square,
        stroke: Stroke::new((side * 2.0 / 24.0).max(1.1), color),
    };

    // Grouped so no single match grows past the point of being readable. Each
    // helper reports whether it recognised the icon.
    let _ = profile_glyph(&pen, icon)
        || solid_glyph(&pen, icon)
        || action_glyph(&pen, icon)
        || view_glyph(&pen, icon);
}

/// Flat things: sketch entities, cuts, and the datum plane.
fn profile_glyph(pen: &Pen<'_>, icon: Icon) -> bool {
    match icon {
        Icon::Pen => {
            pen.path(
                &[
                    (4.0, 20.0),
                    (7.5, 19.0),
                    (19.0, 7.5),
                    (16.5, 5.0),
                    (5.0, 16.5),
                ],
                true,
            );
            pen.line((14.5, 7.0), (17.0, 9.5));
        }
        Icon::Square => pen.rounded_rect(4.0, 5.0, 16.0, 14.0, 2.0),
        Icon::Circle => pen.circle(12.0, 12.0, 8.0),
        Icon::Hexagon => pen.hexagon(12.0, 12.0, 8.5),
        Icon::Slot => {
            pen.rounded_rect(3.0, 8.5, 18.0, 7.0, 3.5);
            pen.dot(12.0, 12.0, 2.0);
        }
        Icon::Hole => {
            pen.circle(12.0, 12.0, 8.0);
            pen.dot(12.0, 12.0, 2.4);
        }
        Icon::HexHole => {
            pen.hexagon(12.0, 12.0, 8.5);
            pen.dot(12.0, 12.0, 2.4);
        }
        Icon::Plane => {
            pen.path(&[(3.0, 9.0), (14.0, 5.0), (21.0, 15.0), (10.0, 19.0)], true);
        }
        _ => return false,
    }
    true
}

/// Solids and the operations that act on them.
fn solid_glyph(pen: &Pen<'_>, icon: Icon) -> bool {
    match icon {
        Icon::Cube => {
            pen.path(
                &[
                    (12.0, 3.0),
                    (20.0, 7.5),
                    (20.0, 16.5),
                    (12.0, 21.0),
                    (4.0, 16.5),
                    (4.0, 7.5),
                ],
                true,
            );
            pen.path(&[(4.0, 7.5), (12.0, 12.0), (20.0, 7.5)], false);
            pen.line((12.0, 12.0), (12.0, 21.0));
        }
        Icon::Sphere => {
            pen.circle(12.0, 12.0, 8.5);
            pen.ellipse(12.0, 12.0, 8.5, 3.4);
        }
        Icon::Cylinder => {
            pen.ellipse(12.0, 6.5, 7.0, 3.0);
            pen.line((5.0, 6.5), (5.0, 17.5));
            pen.line((19.0, 6.5), (19.0, 17.5));
            pen.arc(12.0, 17.5, 7.0, 3.0, 0.0, std::f32::consts::PI);
        }
        Icon::Shell => {
            pen.rounded_rect(2.5, 2.5, 19.0, 19.0, 2.5);
            pen.rounded_rect(8.5, 8.5, 7.0, 7.0, 1.0);
        }
        Icon::Offset => {
            pen.dashed(&[(2.5, 2.5), (21.5, 2.5), (21.5, 21.5), (2.5, 21.5)]);
            pen.rounded_rect(7.0, 7.0, 10.0, 10.0, 1.5);
        }
        Icon::Union => {
            pen.circle(9.0, 12.0, 6.5);
            pen.circle(15.0, 12.0, 6.5);
        }
        Icon::Subtract => {
            pen.circle(9.0, 12.0, 6.5);
            pen.arc(
                15.0,
                12.0,
                6.5,
                6.5,
                -std::f32::consts::FRAC_PI_2,
                std::f32::consts::FRAC_PI_2,
            );
        }
        Icon::Intersect => {
            pen.arc(
                9.0,
                12.0,
                6.5,
                6.5,
                -std::f32::consts::FRAC_PI_3,
                std::f32::consts::FRAC_PI_3,
            );
            pen.arc(
                15.0,
                12.0,
                6.5,
                6.5,
                std::f32::consts::PI - std::f32::consts::FRAC_PI_3,
                std::f32::consts::PI + std::f32::consts::FRAC_PI_3,
            );
        }
        Icon::Torus => {
            pen.ellipse(12.0, 12.0, 9.0, 5.5);
            pen.ellipse(12.0, 12.0, 3.5, 2.0);
        }
        Icon::Extrude => {
            pen.rounded_rect(4.0, 11.0, 13.0, 9.0, 1.5);
            pen.path(&[(4.0, 11.0), (8.0, 6.0), (21.0, 6.0), (17.0, 11.0)], true);
            pen.line((21.0, 6.0), (21.0, 15.0));
            pen.line((17.0, 20.0), (21.0, 15.0));
        }
        Icon::Move => {
            pen.line((12.0, 3.0), (12.0, 21.0));
            pen.line((3.0, 12.0), (21.0, 12.0));
            pen.path(&[(9.0, 6.0), (12.0, 3.0), (15.0, 6.0)], false);
            pen.path(&[(9.0, 18.0), (12.0, 21.0), (15.0, 18.0)], false);
            pen.path(&[(6.0, 9.0), (3.0, 12.0), (6.0, 15.0)], false);
            pen.path(&[(18.0, 9.0), (21.0, 12.0), (18.0, 15.0)], false);
        }
        _ => return false,
    }
    true
}

/// Interface actions rather than geometry.
fn action_glyph(pen: &Pen<'_>, icon: Icon) -> bool {
    match icon {
        Icon::Undo => {
            pen.path(&[(9.0, 7.0), (4.0, 12.0), (9.0, 17.0)], false);
            pen.arc(
                12.0,
                12.0,
                8.0,
                8.0,
                std::f32::consts::PI,
                std::f32::consts::TAU,
            );
            pen.line((4.0, 12.0), (14.0, 12.0));
        }
        Icon::Redo => {
            pen.path(&[(15.0, 7.0), (20.0, 12.0), (15.0, 17.0)], false);
            pen.arc(
                12.0,
                12.0,
                8.0,
                8.0,
                std::f32::consts::PI,
                std::f32::consts::TAU,
            );
            pen.line((10.0, 12.0), (20.0, 12.0));
        }
        Icon::File => {
            pen.path(
                &[
                    (6.0, 3.0),
                    (14.0, 3.0),
                    (19.0, 8.0),
                    (19.0, 21.0),
                    (6.0, 21.0),
                ],
                true,
            );
            pen.path(&[(14.0, 3.0), (14.0, 8.0), (19.0, 8.0)], false);
        }
        Icon::Folder => {
            pen.path(
                &[
                    (3.0, 19.0),
                    (3.0, 6.0),
                    (9.0, 6.0),
                    (11.0, 9.0),
                    (21.0, 9.0),
                    (21.0, 19.0),
                ],
                true,
            );
        }
        Icon::Trash => {
            pen.line((3.5, 6.0), (20.5, 6.0));
            pen.path(&[(9.0, 6.0), (9.0, 3.5), (15.0, 3.5), (15.0, 6.0)], false);
            pen.path(&[(5.5, 6.0), (6.5, 20.5), (17.5, 20.5), (18.5, 6.0)], false);
            pen.line((10.0, 10.0), (10.0, 17.0));
            pen.line((14.0, 10.0), (14.0, 17.0));
        }
        Icon::Save => {
            pen.path(
                &[
                    (4.0, 4.0),
                    (16.0, 4.0),
                    (20.0, 8.0),
                    (20.0, 20.0),
                    (4.0, 20.0),
                ],
                true,
            );
            pen.rounded_rect(8.0, 4.0, 8.0, 5.0, 0.5);
            pen.rounded_rect(7.0, 13.0, 10.0, 7.0, 0.5);
        }
        Icon::Download => {
            pen.line((12.0, 3.0), (12.0, 15.0));
            pen.path(&[(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)], false);
            pen.path(
                &[(4.0, 18.0), (4.0, 21.0), (20.0, 21.0), (20.0, 18.0)],
                false,
            );
        }
        _ => return false,
    }
    true
}

/// Viewport furniture: the pointer, the framing control, the stack of bodies.
fn view_glyph(pen: &Pen<'_>, icon: Icon) -> bool {
    match icon {
        Icon::Help => {
            pen.circle(12.0, 12.0, 9.0);
            // The question mark as two strokes and a dot, so it keeps the same
            // weight as every other glyph instead of depending on a font.
            pen.arc(12.0, 9.5, 3.2, 3.2, 3.4, 6.6);
            pen.line((12.0, 12.7), (12.0, 15.0));
            pen.dot(12.0, 17.8, 1.0);
        }
        Icon::Sun => {
            pen.circle(12.0, 12.0, 4.2);
            // Eight rays on the diagonals and axes, drawn as short segments so
            // they keep the 2px stroke rather than tapering.
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let (s, c) = a.sin_cos();
                pen.line(
                    (12.0 + c * 7.2, 12.0 + s * 7.2),
                    (12.0 + c * 9.8, 12.0 + s * 9.8),
                );
            }
        }
        Icon::Moon => {
            // A crescent as two arcs rather than a filled shape, so it reads at
            // the same weight as every other glyph.
            pen.arc(12.0, 12.0, 9.0, 9.0, 0.6, 4.2);
            pen.arc(15.5, 9.0, 8.4, 8.4, 1.9, 3.6);
        }
        Icon::Monitor => {
            pen.rounded_rect(3.0, 4.0, 18.0, 12.0, 2.0);
            pen.line((8.0, 20.0), (16.0, 20.0));
            pen.line((12.0, 16.0), (12.0, 20.0));
        }
        Icon::Cursor => {
            pen.path(
                &[
                    (5.0, 3.0),
                    (5.0, 19.0),
                    (9.5, 15.0),
                    (12.5, 21.0),
                    (15.0, 19.5),
                    (12.0, 14.0),
                    (18.0, 13.5),
                ],
                true,
            );
        }
        Icon::Frame => {
            pen.path(&[(3.0, 8.0), (3.0, 3.0), (8.0, 3.0)], false);
            pen.path(&[(16.0, 3.0), (21.0, 3.0), (21.0, 8.0)], false);
            pen.path(&[(21.0, 16.0), (21.0, 21.0), (16.0, 21.0)], false);
            pen.path(&[(8.0, 21.0), (3.0, 21.0), (3.0, 16.0)], false);
        }
        Icon::Layers => {
            pen.path(&[(12.0, 3.0), (21.0, 8.0), (12.0, 13.0), (3.0, 8.0)], true);
            pen.path(&[(3.0, 13.0), (12.0, 18.0), (21.0, 13.0)], false);
        }
        _ => return false,
    }
    true
}

/// The glyph that stands for a node kind in the design tree.
pub(crate) fn for_kind(kind: &str) -> Icon {
    match kind {
        "sphere" => Icon::Sphere,
        "box" => Icon::Cube,
        "cylinder" => Icon::Cylinder,
        "torus" => Icon::Torus,
        "plane" => Icon::Plane,
        "union" => Icon::Union,
        "difference" => Icon::Subtract,
        "intersection" => Icon::Intersect,
        "transform" => Icon::Move,
        "offset" => Icon::Offset,
        "extrude" => Icon::Extrude,
        "shell" => Icon::Shell,
        _ => Icon::Layers,
    }
}

/// Draws on a 24 by 24 grid mapped into a rectangle.
struct Pen<'a> {
    painter: &'a Painter,
    rect: Rect,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + Vec2::new(x / 24.0 * self.rect.width(), y / 24.0 * self.rect.height())
    }

    fn scale(&self, v: f32) -> f32 {
        v / 24.0 * self.rect.width()
    }

    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.painter
            .line_segment([self.at(a.0, a.1), self.at(b.0, b.1)], self.stroke);
    }

    fn path(&self, points: &[(f32, f32)], closed: bool) {
        let mut pts: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        if closed {
            pts.push(pts[0]);
        }
        self.painter.add(egui::Shape::line(pts, self.stroke));
    }

    /// A regular hexagon, flat top, matching the application mark's geometry.
    fn hexagon(&self, cx: f32, cy: f32, r: f32) {
        let pts: Vec<(f32, f32)> = (0..6)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_6 + i as f32 * std::f32::consts::FRAC_PI_3;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.path(&pts, true);
    }

    /// A small filled disc. Reads as "drilled through" at interface sizes,
    /// where a second outline would merge with the first.
    fn dot(&self, cx: f32, cy: f32, r: f32) {
        self.painter
            .circle_filled(self.at(cx, cy), self.scale(r), self.stroke.color);
    }

    /// A dashed closed path, for an outline that is implied rather than drawn.
    fn dashed(&self, points: &[(f32, f32)]) {
        let mut pts: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        pts.push(pts[0]);
        let dash = self.scale(2.4);
        self.painter.extend(egui::Shape::dashed_line(
            &pts,
            self.stroke,
            dash,
            dash * 0.8,
        ));
    }

    fn circle(&self, cx: f32, cy: f32, r: f32) {
        self.painter
            .circle_stroke(self.at(cx, cy), self.scale(r), self.stroke);
    }

    fn ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32) {
        self.arc(cx, cy, rx, ry, 0.0, std::f32::consts::TAU);
    }

    fn arc(&self, cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32) {
        const STEPS: usize = 28;
        let pts: Vec<Pos2> = (0..=STEPS)
            .map(|i| {
                let t = from + (to - from) * i as f32 / STEPS as f32;
                self.at(cx + rx * t.cos(), cy + ry * t.sin())
            })
            .collect();
        self.painter.add(egui::Shape::line(pts, self.stroke));
    }

    fn rounded_rect(&self, x: f32, y: f32, width: f32, height: f32, radius: f32) {
        let rect = Rect::from_min_max(self.at(x, y), self.at(x + width, y + height));
        self.painter.rect_stroke(
            rect,
            egui::CornerRadius::same(self.scale(radius).round() as u8),
            self.stroke,
            egui::StrokeKind::Middle,
        );
    }
}
