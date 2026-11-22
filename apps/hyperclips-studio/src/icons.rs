//! Small vector icons drawn with the painter (the bundled fonts lack most
//! symbol glyphs, and vectors stay sharp at every UI scale).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ic {
    Library,
    Highlight,
    Editor,
    Vault,
    Settings,
    Share,
    Search,
    Camera,
    Record,
    Monitor,
    Mic,
    Keyboard,
    Folder,
    Palette,
    Bolt,
    Gamepad,
    Bell,
    Plug,
    Download,
    Save,
    Globe,
    Star,
    StarFilled,
    Play,
    Clock,
    Filter,
    Logo,
}

pub fn paint(p: &Painter, rect: Rect, icon: Ic, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let u = s / 16.0; // design grid: 16x16
    let pt = |x: f32, y: f32| pos2(c.x + (x - 8.0) * u, c.y + (y - 8.0) * u);
    let st = Stroke::new((1.6 * u).max(1.2), color);
    let line = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], st);
    };
    let rrect = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        p.rect_stroke(
            Rect::from_two_pos(pt(x0, y0), pt(x1, y1)),
            r * u,
            st,
            eframe::egui::StrokeKind::Middle,
        );
    };
    let poly = |pts: Vec<Pos2>, closed: bool| {
        if closed {
            p.add(Shape::closed_line(pts, st));
        } else {
            p.add(Shape::line(pts, st));
        }
    };
    match icon {
        Ic::Library => {
            rrect(2.0, 2.0, 7.0, 7.0, 1.5);
            rrect(9.0, 2.0, 14.0, 7.0, 1.5);
            rrect(2.0, 9.0, 7.0, 14.0, 1.5);
            rrect(9.0, 9.0, 14.0, 14.0, 1.5);
        }
        Ic::Highlight => {
            // Four-point sparkle plus a small one.
            poly(
                vec![
                    pt(7.0, 1.5),
                    pt(8.6, 6.0),
                    pt(13.0, 7.5),
                    pt(8.6, 9.0),
                    pt(7.0, 13.5),
                    pt(5.4, 9.0),
                    pt(1.0, 7.5),
                    pt(5.4, 6.0),
                ],
                true,
            );
            line(pt(13.0, 11.0), pt(13.0, 15.0));
            line(pt(11.0, 13.0), pt(15.0, 13.0));
        }
        Ic::Editor => {
            // Scissors.
            p.circle_stroke(pt(4.0, 12.0), 2.2 * u, st);
            p.circle_stroke(pt(12.0, 12.0), 2.2 * u, st);
            line(pt(5.6, 10.4), pt(12.0, 2.0));
            line(pt(10.4, 10.4), pt(4.0, 2.0));
        }
        Ic::Vault => {
            rrect(3.0, 7.0, 13.0, 14.5, 1.5);
            p.add(Shape::line(
                (0..=12)
                    .map(|k| {
                        let a = std::f32::consts::PI * (k as f32 / 12.0);
                        pt(8.0 - 3.2 * a.cos(), 7.0 - 3.8 * a.sin())
                    })
                    .collect(),
                st,
            ));
            p.circle_filled(pt(8.0, 10.5), 1.1 * u, color);
        }
        Ic::Settings => {
            p.circle_stroke(c, 2.4 * u, st);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.cos(), a.sin());
                line(c + d * 4.6 * u, c + d * 6.8 * u);
            }
            p.circle_stroke(c, 4.6 * u, st);
        }
        Ic::Share => {
            p.circle_stroke(pt(12.0, 3.5), 2.0 * u, st);
            p.circle_stroke(pt(4.0, 8.0), 2.0 * u, st);
            p.circle_stroke(pt(12.0, 12.5), 2.0 * u, st);
            line(pt(5.8, 7.0), pt(10.2, 4.5));
            line(pt(5.8, 9.0), pt(10.2, 11.5));
        }
        Ic::Search => {
            p.circle_stroke(pt(7.0, 7.0), 4.5 * u, st);
            line(pt(10.3, 10.3), pt(14.0, 14.0));
        }
        Ic::Camera => {
            rrect(1.5, 4.5, 14.5, 13.5, 2.0);
            p.circle_stroke(pt(8.0, 9.0), 2.6 * u, st);
            line(pt(5.5, 4.5), pt(6.5, 2.5));
            line(pt(6.5, 2.5), pt(9.5, 2.5));
            line(pt(9.5, 2.5), pt(10.5, 4.5));
        }
        Ic::Record => {
            p.circle_filled(c, 4.5 * u, color);
        }
        Ic::Monitor => {
            rrect(1.5, 2.5, 14.5, 11.0, 1.5);
            line(pt(8.0, 11.0), pt(8.0, 14.0));
            line(pt(5.0, 14.0), pt(11.0, 14.0));
        }
        Ic::Mic => {
            rrect(5.5, 1.5, 10.5, 9.5, 2.5);
            p.add(Shape::line(
                (0..=10)
                    .map(|k| {
                        let a = std::f32::consts::PI * (k as f32 / 10.0);
                        pt(8.0 - 4.5 * a.cos(), 8.0 + 3.5 * a.sin())
                    })
                    .collect(),
                st,
            ));
            line(pt(8.0, 11.5), pt(8.0, 14.5));
        }
        Ic::Keyboard => {
            rrect(1.0, 4.0, 15.0, 12.5, 1.5);
            for x in [4.0, 7.0, 10.0, 13.0] {
                p.circle_filled(pt(x - 0.5, 7.0), 0.8 * u, color);
            }
            line(pt(4.5, 10.0), pt(11.5, 10.0));
        }
        Ic::Folder => {
            poly(
                vec![
                    pt(1.5, 3.5),
                    pt(6.0, 3.5),
                    pt(7.5, 5.0),
                    pt(14.5, 5.0),
                    pt(14.5, 13.0),
                    pt(1.5, 13.0),
                ],
                true,
            );
        }
        Ic::Palette => {
            p.circle_stroke(c, 6.2 * u, st);
            for (x, y) in [(5.0, 6.0), (8.0, 4.2), (11.0, 6.0), (11.2, 9.5)] {
                p.circle_filled(pt(x, y), 1.1 * u, color);
            }
            p.circle_filled(pt(6.0, 10.5), 1.6 * u, color);
        }
        Ic::Bolt => {
            poly(
                vec![
                    pt(9.5, 1.0),
                    pt(3.5, 9.0),
                    pt(7.5, 9.0),
                    pt(6.5, 15.0),
                    pt(12.5, 7.0),
                    pt(8.5, 7.0),
                ],
                true,
            );
        }
        Ic::Gamepad => {
            rrect(1.0, 4.5, 15.0, 12.0, 3.5);
            line(pt(4.0, 8.2), pt(7.0, 8.2));
            line(pt(5.5, 6.7), pt(5.5, 9.7));
            p.circle_filled(pt(10.5, 7.5), 0.9 * u, color);
            p.circle_filled(pt(12.3, 9.2), 0.9 * u, color);
        }
        Ic::Bell => {
            poly(
                vec![
                    pt(3.0, 12.0),
                    pt(4.0, 10.0),
                    pt(4.0, 6.5),
                    pt(6.0, 3.2),
                    pt(8.0, 2.6),
                    pt(10.0, 3.2),
                    pt(12.0, 6.5),
                    pt(12.0, 10.0),
                    pt(13.0, 12.0),
                ],
                true,
            );
            line(pt(6.8, 14.2), pt(9.2, 14.2));
        }
        Ic::Plug => {
            line(pt(6.0, 1.5), pt(6.0, 5.0));
            line(pt(10.0, 1.5), pt(10.0, 5.0));
            poly(
                vec![
                    pt(3.5, 5.0),
                    pt(12.5, 5.0),
                    pt(12.0, 9.0),
                    pt(9.5, 11.0),
                    pt(6.5, 11.0),
                    pt(4.0, 9.0),
                ],
                true,
            );
            line(pt(8.0, 11.0), pt(8.0, 15.0));
        }
        Ic::Download => {
            line(pt(8.0, 1.5), pt(8.0, 10.0));
            poly(vec![pt(4.5, 6.8), pt(8.0, 10.3), pt(11.5, 6.8)], false);
            poly(
                vec![pt(2.0, 11.0), pt(2.0, 14.0), pt(14.0, 14.0), pt(14.0, 11.0)],
                false,
            );
        }
        Ic::Save => {
            poly(
                vec![
                    pt(2.0, 2.0),
                    pt(11.5, 2.0),
                    pt(14.0, 4.5),
                    pt(14.0, 14.0),
                    pt(2.0, 14.0),
                ],
                true,
            );
            rrect(5.0, 2.0, 10.5, 5.5, 0.5);
            rrect(4.5, 9.0, 11.5, 14.0, 0.5);
        }
        Ic::Globe => {
            p.circle_stroke(c, 6.3 * u, st);
            line(pt(1.8, 8.0), pt(14.2, 8.0));
            p.add(Shape::closed_line(
                (0..24)
                    .map(|k| {
                        let a = std::f32::consts::TAU * (k as f32 / 24.0);
                        pt(8.0 + 2.8 * a.cos(), 8.0 + 6.3 * a.sin())
                    })
                    .collect(),
                st,
            ));
        }
        Ic::Star | Ic::StarFilled => {
            let pts: Vec<Pos2> = (0..10)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                    let r = if k % 2 == 0 { 6.8 } else { 2.9 };
                    pt(8.0 + r * a.cos(), 8.6 + r * a.sin())
                })
                .collect();
            if icon == Ic::StarFilled {
                // Fan of triangles from the centre (the star is not convex).
                for k in 0..10 {
                    p.add(Shape::convex_polygon(
                        vec![pt(8.0, 8.6), pts[k], pts[(k + 1) % 10]],
                        color,
                        Stroke::NONE,
                    ));
                }
            } else {
                p.add(Shape::closed_line(pts, st));
            }
        }
        Ic::Clock => {
            p.circle_stroke(c, 6.3 * u, st);
            line(pt(8.0, 4.2), pt(8.0, 8.0));
            line(pt(8.0, 8.0), pt(10.8, 9.8));
        }
        Ic::Filter => {
            line(pt(2.0, 4.0), pt(14.0, 4.0));
            line(pt(4.5, 8.0), pt(11.5, 8.0));
            line(pt(6.5, 12.0), pt(9.5, 12.0));
        }
        Ic::Logo => {
            // Hexagon like Hyper Launcher's mark, with a play triangle.
            let hex: Vec<Pos2> = (0..6)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::FRAC_PI_3;
                    pt(8.0 + 7.0 * a.cos(), 8.0 + 7.0 * a.sin())
                })
                .collect();
            p.add(Shape::convex_polygon(
                hex.clone(),
                color.gamma_multiply(0.25),
                Stroke::NONE,
            ));
            p.add(Shape::closed_line(hex, Stroke::new(1.9 * u, color)));
            p.add(Shape::convex_polygon(
                vec![pt(6.3, 5.0), pt(11.2, 8.0), pt(6.3, 11.0)],
                color,
                Stroke::NONE,
            ));
        }
        Ic::Play => {
            p.add(Shape::convex_polygon(
                vec![pt(4.5, 2.5), pt(13.5, 8.0), pt(4.5, 13.5)],
                color,
                Stroke::NONE,
            ));
        }
    }
}

/// Allocate a square and draw an icon in it.
pub fn icon(
    ui: &mut eframe::egui::Ui,
    icon: Ic,
    size: f32,
    color: Color32,
) -> eframe::egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), eframe::egui::Sense::hover());
    paint(ui.painter(), rect, icon, color);
    resp
}
