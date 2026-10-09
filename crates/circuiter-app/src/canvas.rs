//! The schematic canvas: camera, grid, drawing and picking.

use circuiter_model::connectivity::Item;
use circuiter_scene::{
    HAlign, LINE_WIDTH, Owner, Pt, Rect, Rgba, Role, Scene, Shape, Theme, VAlign,
};
use eframe::egui::{
    self, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Sense, Stroke, Vec2,
    epaint::TextShape,
};

const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 2000.0;
/// Text smaller than this many pixels is skipped; it would be unreadable noise.
const MIN_TEXT_PX: f32 = 4.0;

/// Maps sheet millimetres to screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Sheet point at the centre of the view.
    pub center: Pt,
    /// Pixels per millimetre.
    pub scale: f32,
}

impl Camera {
    pub fn fit(bounds: &Rect, view: egui::Rect) -> Camera {
        if bounds.is_empty() {
            return Camera { center: Pt::new(0.0, 0.0), scale: 10.0 };
        }
        let b = bounds.expand(5.08);
        let sx = view.width() / b.width() as f32;
        let sy = view.height() / b.height() as f32;
        Camera { center: b.center(), scale: sx.min(sy).clamp(MIN_SCALE, 60.0) }
    }

    pub fn to_screen(self, view: egui::Rect, p: Pt) -> Pos2 {
        view.center()
            + Vec2::new(
                (p.x - self.center.x) as f32 * self.scale,
                (p.y - self.center.y) as f32 * self.scale,
            )
    }

    pub fn to_world(self, view: egui::Rect, p: Pos2) -> Pt {
        let d = (p - view.center()) / self.scale;
        Pt::new(self.center.x + d.x as f64, self.center.y + d.y as f64)
    }

    fn zoom_about(&mut self, view: egui::Rect, anchor: Pos2, factor: f32) {
        let before = self.to_world(view, anchor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let d = (anchor - view.center()) / self.scale;
        self.center = Pt::new(before.x - d.x as f64, before.y - d.y as f64);
    }
}

/// What to emphasise when drawing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Highlight {
    pub selection: Option<Owner>,
    /// Index into the scene's connectivity nets.
    pub net: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CanvasEvent {
    Select(Option<Owner>),
    Descend(usize),
}

#[derive(Debug, Default)]
pub struct CanvasOutput {
    pub events: Vec<CanvasEvent>,
    /// Cursor position on the sheet, if the pointer is over the canvas.
    pub cursor: Option<Pt>,
    pub hovered: Option<Owner>,
}

pub fn color(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

/// Draws `scene` filling the available space and handles navigation input.
/// `camera` is fitted to the scene on first use.
pub fn show(
    ui: &mut egui::Ui,
    scene: &Scene,
    camera: &mut Option<Camera>,
    theme: &Theme,
    highlight: Highlight,
    show_grid: bool,
) -> CanvasOutput {
    let view = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(view, Sense::click_and_drag());
    let cam = camera.get_or_insert_with(|| Camera::fit(&scene.bounds, view));
    let mut out = CanvasOutput::default();

    // Navigation: wheel zooms about the cursor (as in most EDA tools), horizontal
    // scroll pans, middle/right drag pans, as does left drag with Space held.
    if let Some(hover) = response.hover_pos() {
        let (scroll, zoom, space) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.key_down(egui::Key::Space)));
        let factor = zoom * (scroll.y * 0.0018).exp();
        if factor != 1.0 {
            cam.zoom_about(view, hover, factor);
        }
        if scroll.x != 0.0 {
            cam.center.x -= (scroll.x / cam.scale) as f64;
        }
        let panning = response.dragged_by(PointerButton::Middle)
            || response.dragged_by(PointerButton::Secondary)
            || (space && response.dragged_by(PointerButton::Primary));
        if panning {
            let d = response.drag_delta() / cam.scale;
            cam.center = Pt::new(cam.center.x - d.x as f64, cam.center.y - d.y as f64);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        let world = cam.to_world(view, hover);
        out.cursor = Some(world);
        out.hovered = scene.hit_test(world, pick_tolerance(cam));
    }

    if response.clicked() || response.double_clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let hit = scene.hit_test(cam.to_world(view, pos), pick_tolerance(cam));
            out.events.push(CanvasEvent::Select(hit));
            if response.double_clicked() {
                if let Some(Owner::Instance(i)) = hit {
                    if scene.descendable[i] {
                        out.events.push(CanvasEvent::Descend(i));
                    }
                }
            }
        }
    }

    let painter = ui.painter_at(view);
    painter.rect_filled(view, 0.0, color(theme.background));
    if show_grid {
        draw_grid(&painter, view, cam, theme);
    }
    draw_scene(&painter, view, cam, scene, theme, highlight, out.hovered);
    out
}

fn pick_tolerance(cam: &Camera) -> f64 {
    (5.0 / cam.scale) as f64
}

fn draw_grid(painter: &Painter, view: egui::Rect, cam: &Camera, theme: &Theme) {
    // 100 mil grid, falling back to 1 inch when zoomed far out.
    let Some(spacing) = [2.54, 25.4].into_iter().find(|s| s * cam.scale as f64 >= 10.0) else {
        return;
    };
    let tl = cam.to_world(view, view.left_top());
    let br = cam.to_world(view, view.right_bottom());
    let (x0, x1) = ((tl.x / spacing).floor() as i64, (br.x / spacing).ceil() as i64);
    let (y0, y1) = ((tl.y / spacing).floor() as i64, (br.y / spacing).ceil() as i64);
    if (x1 - x0) * (y1 - y0) > 80_000 {
        return;
    }
    let base = color(theme.grid);
    let minor = base.gamma_multiply(0.8);
    let major_every = (25.4 / spacing).round() as i64;
    for gx in x0..=x1 {
        for gy in y0..=y1 {
            let p = cam.to_screen(view, Pt::new(gx as f64 * spacing, gy as f64 * spacing));
            let major = major_every > 1 && gx % major_every == 0 && gy % major_every == 0;
            let (r, c) = if major { (1.4, base) } else { (0.9, minor) };
            painter.circle_filled(p, r, c);
        }
    }
}

fn draw_scene(
    painter: &Painter,
    view: egui::Rect,
    cam: &Camera,
    scene: &Scene,
    theme: &Theme,
    highlight: Highlight,
    hovered: Option<Owner>,
) {
    let px = |mm: f64| (mm as f32 * cam.scale).max(1.0);
    let conn = &scene.connectivity;
    let net_of_wire = |i: usize| conn.net_of(Item::Wire(i));
    let highlight_color = color(theme.highlight);
    let selection_color = color(theme.selection);

    // Hover and selection outlines go underneath the drawing.
    for (owner, fill) in [(hovered, false), (highlight.selection, true)] {
        if let Some(Owner::Instance(i)) = owner {
            let r = scene.instance_bounds[i].expand(0.635);
            let rect =
                egui::Rect::from_two_pos(cam.to_screen(view, r.min), cam.to_screen(view, r.max));
            let c = if fill { selection_color } else { color(theme.grid) };
            painter.rect_filled(rect, 3.0, c.gamma_multiply(if fill { 0.12 } else { 0.18 }));
        }
    }

    let mut highlighted_segments: Vec<(Pt, Pt)> = Vec::new();
    for e in &scene.elements {
        let b = e.shape.bounds();
        let screen =
            egui::Rect::from_two_pos(cam.to_screen(view, b.min), cam.to_screen(view, b.max));
        if !screen.expand(4.0).intersects(view) {
            continue;
        }
        let selected = highlight.selection.is_some_and(|s| s == e.owner);
        let in_net = match e.owner {
            Owner::Wire(i) => highlight.net.is_some() && net_of_wire(i) == highlight.net,
            _ => false,
        };
        let (c, boost) = if selected {
            (selection_color, 1.8)
        } else if in_net {
            (highlight_color, 1.8)
        } else {
            (color(theme.color(e.role)), 1.0)
        };
        let width = if e.role == Role::Wire { LINE_WIDTH * 1.2 } else { LINE_WIDTH };
        let stroke = Stroke::new(px(width) * boost, c);

        match &e.shape {
            Shape::Polyline { points, closed, filled } => {
                let pts: Vec<Pos2> = points.iter().map(|&p| cam.to_screen(view, p)).collect();
                if in_net {
                    highlighted_segments.extend(points.windows(2).map(|s| (s[0], s[1])));
                }
                let shape = match (closed, filled) {
                    (true, true) => egui::Shape::convex_polygon(pts, c, stroke),
                    (true, false) => egui::Shape::closed_line(pts, stroke),
                    _ => egui::Shape::line(pts, stroke),
                };
                painter.add(shape);
            }
            Shape::Circle { center, radius, filled } => {
                let center = cam.to_screen(view, *center);
                let r = (*radius as f32 * cam.scale).max(1.5);
                if *filled {
                    painter.circle_filled(center, r, c);
                } else {
                    painter.circle_stroke(center, r, stroke);
                }
            }
            Shape::Text { at, text, size, halign, valign, vertical } => {
                let size_px = *size as f32 * cam.scale;
                if size_px >= MIN_TEXT_PX {
                    draw_text(
                        painter,
                        cam.to_screen(view, *at),
                        text,
                        size_px,
                        *halign,
                        *valign,
                        *vertical,
                        c,
                    );
                }
            }
        }
    }

    // Junctions and pins on the highlighted net.
    if let Some(net) = highlight.net {
        for &j in &conn.junctions {
            let p = Pt::from(j);
            if highlighted_segments.iter().any(|&(a, b)| on_segment(p, a, b)) {
                let r = (circuiter_scene::JUNCTION_RADIUS as f32 * cam.scale).max(2.0) * 1.2;
                painter.circle_filled(cam.to_screen(view, p), r, highlight_color);
            }
        }
        for item in &conn.nets[net].items {
            if let Item::Pin(i) = *item {
                let p = cam.to_screen(view, conn.pins[i].at.into());
                painter.circle_stroke(
                    p,
                    (0.5 * cam.scale).max(3.0),
                    Stroke::new(1.5, highlight_color),
                );
            }
        }
    }
}

fn on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let within = p.x >= a.x.min(b.x) - 1e-6
        && p.x <= a.x.max(b.x) + 1e-6
        && p.y >= a.y.min(b.y) - 1e-6
        && p.y <= a.y.max(b.y) + 1e-6;
    cross.abs() < 1e-6 && within
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    painter: &Painter,
    at: Pos2,
    text: &str,
    size_px: f32,
    halign: HAlign,
    valign: VAlign,
    vertical: bool,
    c: Color32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::monospace(size_px), c);
    let size = galley.size();
    // The anchor point within the unrotated galley.
    let au = match halign {
        HAlign::Left => 0.0,
        HAlign::Center => size.x / 2.0,
        HAlign::Right => size.x,
    };
    let av = match valign {
        VAlign::Top => 0.0,
        VAlign::Center => size.y / 2.0,
        VAlign::Bottom => size.y,
    };
    if vertical {
        // Rotating 90° counter-clockwise maps galley (u, v) to screen (v, -u).
        let pos = Pos2::new(at.x - av, at.y + au);
        painter.add(TextShape::new(pos, galley, c).with_angle(-std::f32::consts::FRAC_PI_2));
    } else {
        painter.add(TextShape::new(at - Vec2::new(au, av), galley, c));
    }
}
