//! The schematic canvas: camera, grid, drawing and picking.

use circuiter_model::connectivity::Item;
use circuiter_scene::{
    Element, HAlign, LINE_WIDTH, Owner, Pt, Rect, Rgba, Role, Scene, Shape, Theme, VAlign,
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
    /// Fitted to the sheet automatically and not moved since, so it refits when the
    /// canvas changes size.
    pub fitted: Option<egui::Vec2>,
}

impl Camera {
    pub fn fit(bounds: &Rect, view: egui::Rect) -> Camera {
        if bounds.is_empty() {
            return Camera { center: Pt::new(0.0, 0.0), scale: 10.0, fitted: Some(view.size()) };
        }
        let b = bounds.expand(5.08);
        let sx = view.width() / b.width() as f32;
        let sy = view.height() / b.height() as f32;
        Camera {
            center: b.center(),
            scale: sx.min(sy).clamp(MIN_SCALE, 60.0),
            fitted: Some(view.size()),
        }
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

    pub fn zoom_about(&mut self, view: egui::Rect, anchor: Pos2, factor: f32) {
        let before = self.to_world(view, anchor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let d = (anchor - view.center()) / self.scale;
        self.center = Pt::new(before.x - d.x as f64, before.y - d.y as f64);
    }
}

/// What to emphasise when drawing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Highlight<'a> {
    pub selection: Option<&'a std::collections::BTreeSet<Owner>>,
    /// Index into the scene's connectivity nets.
    pub net: Option<usize>,
}

impl Highlight<'_> {
    fn selected(&self, owner: Owner) -> bool {
        self.selection.is_some_and(|s| s.contains(&owner))
    }
}

/// Things drawn on top of the sheet while editing.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    /// A part following the cursor while placing.
    pub ghost: Vec<Element>,
    /// A wire being drawn.
    pub wire: Vec<Pt>,
    /// The rubber-band selection rectangle.
    pub band: Option<(Pt, Pt)>,
    /// The grid point the next click will use.
    pub snap: Option<Pt>,
    /// Small tags pinned to sheet points, such as simulated net voltages.
    pub notes: Vec<(Pt, String)>,
    /// Parts glowing with heat: their outline and how close they came to destruction,
    /// from 0 (ambient) to 1.
    pub heat: Vec<(Rect, f32)>,
    /// Destroyed parts: where to mark them and what happened.
    pub damage: Vec<(Pt, String)>,
    /// Parts giving light (LEDs, lamps): their body, how brightly (0 to 1, more when
    /// overdriven) and in what colour.
    pub glow: Vec<(Rect, f32, [u8; 3])>,
}

/// Pointer gestures on the sheet, in sheet coordinates. Navigation (zoom, pan) is
/// handled by the canvas itself and not reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Click { at: Pt, shift: bool },
    DoubleClick { at: Pt },
    DragStart { from: Pt },
    Drag { from: Pt, to: Pt },
    DragEnd { from: Pt, to: Pt, shift: bool },
}

/// State the canvas keeps between frames.
#[derive(Debug, Default)]
pub struct CanvasState {
    /// The hand tool is in use: left drag pans the view.
    pub hand: bool,
    drag_from: Option<Pt>,
    /// Where the canvas was drawn last frame, on screen.
    pub view: Option<egui::Rect>,
}

#[derive(Debug, Default)]
pub struct CanvasOutput {
    pub gestures: Vec<Gesture>,
    /// Cursor position on the sheet, if the pointer is over the canvas.
    pub cursor: Option<Pt>,
    pub hovered: Option<Owner>,
    /// How close, in millimetres, counts as "on" an object at the current zoom.
    pub tolerance: f64,
}

pub fn color(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

/// Draws `scene` filling the available space, handles zoom and pan, and reports pointer
/// gestures. `camera` is fitted to the scene on first use.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    scene: &Scene,
    camera: &mut Option<Camera>,
    state: &mut CanvasState,
    theme: &Theme,
    highlight: Highlight<'_>,
    overlay: &Overlay,
    show_grid: bool,
) -> CanvasOutput {
    let view = ui.available_rect_before_wrap();
    state.view = Some(view);
    let response = ui.allocate_rect(view, Sense::click_and_drag());
    let cam = camera.get_or_insert_with(|| Camera::fit(&scene.bounds, view));
    if cam.fitted.is_some_and(|size| size != view.size()) {
        *cam = Camera::fit(&scene.bounds, view);
    }
    let mut out = CanvasOutput { tolerance: pick_tolerance(cam), ..CanvasOutput::default() };
    let (scroll, zoom, space, shift) = ui.input(|i| {
        (i.smooth_scroll_delta, i.zoom_delta(), i.key_down(egui::Key::Space), i.modifiers.shift)
    });

    // Navigation: wheel zooms about the cursor (as in most EDA tools), horizontal
    // scroll pans, middle/right drag pans, as does left drag with Space held.
    // With the hand tool, a plain left drag pans too.
    let hand = state.hand;
    let space = space || hand;
    let panning = response.dragged_by(PointerButton::Middle)
        || response.dragged_by(PointerButton::Secondary)
        || (space && response.dragged_by(PointerButton::Primary));
    if let Some(hover) = response.hover_pos() {
        let factor = zoom * (scroll.y * 0.0018).exp();
        if factor != 1.0 {
            cam.zoom_about(view, hover, factor);
            cam.fitted = None;
        }
        if scroll.x != 0.0 {
            cam.center.x -= (scroll.x / cam.scale) as f64;
            cam.fitted = None;
        }
        ui.ctx().set_cursor_icon(if panning {
            CursorIcon::Grabbing
        } else if hand {
            CursorIcon::Grab
        } else {
            CursorIcon::Crosshair
        });
        let world = cam.to_world(view, hover);
        out.cursor = Some(world);
        out.hovered = scene.hit_test(world, out.tolerance);
    } else if response.contains_pointer() {
        // Something dragged in from elsewhere (a part from the library) is over the
        // sheet: egui doesn't count that as hovering, but where it is still matters.
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            out.cursor = Some(cam.to_world(view, p));
        }
    }
    if panning {
        let d = response.drag_delta() / cam.scale;
        cam.center = Pt::new(cam.center.x - d.x as f64, cam.center.y - d.y as f64);
        cam.fitted = None;
    }

    let world_at = |p: Option<Pos2>| p.map(|p| cam.to_world(view, p));
    if !space {
        if response.drag_started_by(PointerButton::Primary) {
            if let Some(from) = world_at(ui.input(|i| i.pointer.press_origin())) {
                state.drag_from = Some(from);
                out.gestures.push(Gesture::DragStart { from });
            }
        }
        if let (Some(from), Some(to)) = (state.drag_from, world_at(response.interact_pointer_pos()))
        {
            if response.dragged_by(PointerButton::Primary) {
                out.gestures.push(Gesture::Drag { from, to });
            }
        }
    }
    if response.drag_stopped() {
        if let Some(from) = state.drag_from.take() {
            let to =
                world_at(response.interact_pointer_pos().or(response.hover_pos())).unwrap_or(from);
            out.gestures.push(Gesture::DragEnd { from, to, shift });
        }
    }
    if response.clicked() {
        if let Some(at) = world_at(response.interact_pointer_pos()) {
            out.gestures.push(Gesture::Click { at, shift });
        }
    }
    if response.double_clicked() {
        if let Some(at) = world_at(response.interact_pointer_pos()) {
            out.gestures.push(Gesture::DoubleClick { at });
        }
    }

    let painter = ui.painter_at(view);
    painter.rect_filled(view, 0.0, color(theme.background));
    if show_grid {
        draw_grid(&painter, view, cam, theme);
    }
    draw_scene(&painter, view, cam, scene, theme, highlight, out.hovered);
    draw_overlay(&painter, view, cam, theme, overlay);
    out
}

/// Draws a part's symbol as an icon filling `rect`: body and pins only, no text.
pub fn draw_icon(painter: &Painter, rect: egui::Rect, scene: &Scene, c: Color32) {
    let shown = |e: &&Element| matches!(e.role, Role::Body | Role::Pin);
    let mut bounds = Rect::EMPTY;
    for e in scene.elements.iter().filter(shown) {
        bounds.union(&e.shape.bounds());
    }
    if bounds.is_empty() {
        return;
    }
    let scale = (rect.width() / bounds.width().max(1.0) as f32)
        .min(rect.height() / bounds.height().max(1.0) as f32);
    let cam = Camera { center: bounds.center(), scale, fitted: None };
    // Thicker than on the sheet, or the lines vanish at icon size.
    let stroke = Stroke::new(1.3, c);
    for e in scene.elements.iter().filter(shown) {
        draw_element(painter, rect, &cam, e, c, stroke);
    }
}

fn draw_overlay(
    painter: &Painter,
    view: egui::Rect,
    cam: &Camera,
    theme: &Theme,
    overlay: &Overlay,
) {
    let px = |mm: f64| (mm as f32 * cam.scale).max(1.0);
    for e in &overlay.ghost {
        let c = color(theme.color(e.role)).gamma_multiply(0.65);
        draw_element(painter, view, cam, e, c, Stroke::new(px(LINE_WIDTH), c));
    }
    if overlay.wire.len() >= 2 {
        let c = color(theme.wire);
        let pts: Vec<Pos2> = overlay.wire.iter().map(|&p| cam.to_screen(view, p)).collect();
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(px(LINE_WIDTH * 1.2), c)));
        for p in pts {
            painter.circle_filled(p, 2.5, c);
        }
    }
    if let Some((a, b)) = overlay.band {
        let rect = egui::Rect::from_two_pos(cam.to_screen(view, a), cam.to_screen(view, b));
        let c = color(theme.highlight);
        painter.rect_filled(rect, 0.0, c.gamma_multiply(0.12));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
    }
    for (r, level) in &overlay.heat {
        let rect = egui::Rect::from_two_pos(cam.to_screen(view, r.min), cam.to_screen(view, r.max))
            .expand(6.0);
        // Warm yellow through orange to red as the part nears destruction.
        let l = level.clamp(0.0, 1.0);
        let c = Color32::from_rgb(255, (220.0 - 180.0 * l) as u8, (60.0 - 40.0 * l) as u8);
        painter.rect_filled(rect, 6.0, c.gamma_multiply(0.12 + 0.3 * l));
    }
    for (r, level, [cr, cg, cb]) in &overlay.glow {
        let body = egui::Rect::from_two_pos(cam.to_screen(view, r.min), cam.to_screen(view, r.max));
        let c = Color32::from_rgb(*cr, *cg, *cb);
        let l = level.clamp(0.0, 1.5);
        let center = body.center();
        // At least this big on screen, so a light is seen even zoomed far out.
        let size = body.width().max(body.height()).max(18.0);
        // A soft halo: rings fading outwards, wider the brighter it is.
        let reach = size * (0.9 + 0.9 * l.min(1.0));
        for k in (0..8).rev() {
            let t = k as f32 / 7.0;
            let radius = size * 0.35 + (reach - size * 0.35) * t;
            painter.circle_filled(center, radius, c.gamma_multiply(0.16 * (1.0 - t) * l.min(1.0)));
        }
        // The bright core, whiter when overdriven.
        let core = if l > 1.0 { Color32::WHITE } else { c };
        painter.circle_filled(center, size * 0.28, core.gamma_multiply(0.75 * l.min(1.0)));
    }
    for (at, text) in &overlay.damage {
        let p = cam.to_screen(view, *at);
        let c = color(theme.unconnected);
        let galley =
            painter.layout_no_wrap(format!("✖ {text}"), FontId::proportional(12.0), Color32::WHITE);
        let rect = egui::Rect::from_min_size(
            p + Vec2::new(-4.0, -galley.size().y - 6.0),
            galley.size() + Vec2::new(10.0, 4.0),
        );
        painter.rect_filled(rect, 4.0, c.gamma_multiply(0.92));
        painter.galley(rect.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
    }
    // Zoomed far out, the tags would bury the drawing.
    let notes: &[(Pt, String)] = if cam.scale < 3.0 { &[] } else { &overlay.notes };
    for (at, text) in notes {
        let p = cam.to_screen(view, *at);
        if !view.contains(p) {
            continue;
        }
        let c = color(theme.highlight);
        let galley = painter.layout_no_wrap(text.clone(), FontId::monospace(11.0), c);
        let rect = egui::Rect::from_center_size(p, galley.size() + Vec2::new(8.0, 2.0));
        painter.rect_filled(rect, 3.0, color(theme.background).gamma_multiply(0.92));
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, c.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, c);
    }
    if let Some(p) = overlay.snap {
        let p = cam.to_screen(view, p);
        let c = color(theme.highlight);
        let r = 6.0;
        painter.line_segment([p - Vec2::X * r, p + Vec2::X * r], Stroke::new(1.0, c));
        painter.line_segment([p - Vec2::Y * r, p + Vec2::Y * r], Stroke::new(1.0, c));
    }
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
    highlight: Highlight<'_>,
    hovered: Option<Owner>,
) {
    let px = |mm: f64| (mm as f32 * cam.scale).max(1.0);
    let conn = &scene.connectivity;
    let net_of_wire = |i: usize| conn.net_of(Item::Wire(i));
    let highlight_color = color(theme.highlight);
    let selection_color = color(theme.selection);

    // Hover and selection outlines go underneath the drawing.
    let hovered_instance = match hovered {
        Some(Owner::Instance(i)) => Some((i, false)),
        _ => None,
    };
    let selected_instances = highlight.selection.into_iter().flatten().filter_map(|o| match o {
        Owner::Instance(i) => Some((*i, true)),
        _ => None,
    });
    for (i, selected) in hovered_instance.into_iter().chain(selected_instances) {
        let Some(r) = scene.instance_bounds.get(i) else { continue };
        let r = r.expand(0.635);
        let rect = egui::Rect::from_two_pos(cam.to_screen(view, r.min), cam.to_screen(view, r.max));
        let c = if selected { selection_color } else { color(theme.grid) };
        painter.rect_filled(rect, 3.0, c.gamma_multiply(if selected { 0.12 } else { 0.18 }));
    }

    let mut highlighted_segments: Vec<(Pt, Pt)> = Vec::new();
    for e in &scene.elements {
        let b = e.shape.bounds();
        let screen =
            egui::Rect::from_two_pos(cam.to_screen(view, b.min), cam.to_screen(view, b.max));
        if !screen.expand(4.0).intersects(view) {
            continue;
        }
        let selected = highlight.selected(e.owner);
        let in_net = match e.owner {
            Owner::Wire(i) => highlight.net.is_some() && net_of_wire(i) == highlight.net,
            _ => false,
        };
        let hover = hovered == Some(e.owner) && !matches!(e.owner, Owner::Instance(_));
        let (c, boost) = if selected {
            (selection_color, 1.8)
        } else if in_net {
            (highlight_color, 1.8)
        } else if hover {
            (color(theme.color(e.role)), 1.8)
        } else {
            (color(theme.color(e.role)), 1.0)
        };
        let width = if e.role == Role::Wire { LINE_WIDTH * 1.2 } else { LINE_WIDTH };
        let stroke = Stroke::new(px(width) * boost, c);

        if in_net {
            if let Shape::Polyline { points, .. } = &e.shape {
                highlighted_segments.extend(points.windows(2).map(|s| (s[0], s[1])));
            }
        }
        draw_element(painter, view, cam, e, c, stroke);
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

fn draw_element(
    painter: &Painter,
    view: egui::Rect,
    cam: &Camera,
    e: &Element,
    c: Color32,
    stroke: Stroke,
) {
    match &e.shape {
        Shape::Polyline { points, closed, filled } => {
            let pts: Vec<Pos2> = points.iter().map(|&p| cam.to_screen(view, p)).collect();
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
                let at = cam.to_screen(view, *at);
                draw_text(painter, at, text, size_px, *halign, *valign, *vertical, c);
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
