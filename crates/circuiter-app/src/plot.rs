//! Plots of simulation results: waveforms over time, and Bode plots over a
//! logarithmic frequency axis. Cursor readout, zoom and pan.

use crate::canvas::color;
use crate::simulation::show;
use circuiter_scene::Theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

/// Trace colours, chosen to stay distinct on both themes.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(79, 179, 255),
    Color32::from_rgb(255, 160, 64),
    Color32::from_rgb(92, 203, 126),
    Color32::from_rgb(232, 98, 160),
    Color32::from_rgb(180, 142, 219),
    Color32::from_rgb(230, 200, 70),
    Color32::from_rgb(90, 210, 210),
    Color32::from_rgb(226, 114, 91),
];

pub fn trace_color(k: usize) -> Color32 {
    PALETTE[k % PALETTE.len()]
}

pub struct Trace<'a> {
    pub name: &'a str,
    pub x: &'a [f64],
    pub y: &'a [f64],
}

/// A horizontal axis: its unit, its full range, and whether it is logarithmic.
#[derive(Debug, Clone, Copy)]
pub struct XAxis<'a> {
    pub unit: &'a str,
    pub range: (f64, f64),
    pub log: bool,
}

impl XAxis<'_> {
    fn position(self, x: f64) -> f64 {
        if self.log { x.max(f64::MIN_POSITIVE).log10() } else { x }
    }

    fn value_at(self, u: f64) -> f64 {
        if self.log { 10f64.powf(u) } else { u }
    }
}

/// What a plot remembers between frames: the visible part of the x axis, in axis units
/// (log₁₀ of the value on a log axis). Plots stacked over one x axis share a state.
#[derive(Debug, Default)]
pub struct PlotState {
    window: Option<(f64, f64)>,
}

/// "Nice" tick spacing: 1, 2 or 5 times a power of ten, about `count` ticks per span.
fn ticks(lo: f64, hi: f64, count: f64) -> Vec<f64> {
    let span = hi - lo;
    if span.is_nan() || span <= 0.0 || !span.is_finite() {
        return Vec::new();
    }
    let raw = span / count.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    let step =
        [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(raw);
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi + step * 1e-9 && out.len() < 100 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

/// Ticks for an angle axis: multiples of 15°, 30°, 45° or 90°.
fn degree_ticks(lo: f64, hi: f64, count: f64) -> Vec<f64> {
    let raw = (hi - lo) / count.max(1.0);
    let step = [15.0, 30.0, 45.0, 90.0, 180.0].into_iter().find(|s| *s >= raw).unwrap_or(360.0);
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi && out.len() < 100 {
        out.push(t);
        t += step;
    }
    out
}

/// Removes ±360° jumps so a phase curve reads continuously. Points without a phase
/// (NaN, where there is no signal) are skipped.
pub fn unwrap_phase(degrees: &mut [f64]) {
    let mut last: Option<f64> = None;
    for p in degrees.iter_mut().filter(|p| p.is_finite()) {
        if let Some(prev) = last {
            *p -= 360.0 * ((*p - prev) / 360.0).round();
        }
        last = Some(*p);
    }
}

/// A value with its unit. Decibels and degrees read best without SI prefixes.
fn label(v: f64, unit: &str) -> String {
    match unit {
        "dB" | "°" | "°C" => {
            let v = if v.abs() < 1e-9 { 0.0 } else { v };
            if v.fract() == 0.0 { format!("{v:.0}{unit}") } else { format!("{v:.1}{unit}") }
        }
        _ => show(v, unit),
    }
}

/// Draws a plot in `rect`.
pub fn show_plot(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &mut PlotState,
    traces: &[Trace<'_>],
    axis: XAxis<'_>,
    y_unit: &str,
    theme: &Theme,
) {
    draw(ui, rect, state, traces, axis, y_unit, theme, true, 0);
}

/// Draws each trace in its own lane, stacked over one shared time axis, as an
/// oscilloscope or logic analyser does: overlapping logic signals stay readable. With
/// one or two traces it is a single plot.
pub fn show_lanes(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &mut PlotState,
    traces: &[Trace<'_>],
    axis: XAxis<'_>,
    y_unit: &str,
    theme: &Theme,
) {
    let n = traces.len();
    if n <= 2 {
        return show_plot(ui, rect, state, traces, axis, y_unit, theme);
    }
    // The bottom lane also carries the time labels.
    let lane = (rect.height() - 18.0) / n as f32;
    for (k, trace) in traces.iter().enumerate() {
        let top = rect.top() + lane * k as f32;
        let last = k + 1 == n;
        let bottom = if last { rect.bottom() } else { top + lane };
        let r = Rect::from_min_max(Pos2::new(rect.left(), top), Pos2::new(rect.right(), bottom));
        draw(ui, r, state, std::slice::from_ref(trace), axis, y_unit, theme, last, k);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &mut PlotState,
    traces: &[Trace<'_>],
    axis: XAxis<'_>,
    y_unit: &str,
    theme: &Theme,
    x_labels: bool,
    first_color: usize,
) {
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let fg = color(theme.net_label);
    let dim = color(theme.grid);
    painter.rect_filled(rect, 0.0, color(theme.background));
    let bottom = if x_labels { 22.0 } else { 3.0 };
    let plot =
        Rect::from_min_max(rect.min + Vec2::new(64.0, 4.0), rect.max - Vec2::new(12.0, bottom));
    if plot.width() < 40.0 || plot.height() < 16.0 {
        return;
    }

    // Visible window: wheel zooms about the cursor, drag pans, double-click shows all.
    let full = (axis.position(axis.range.0), axis.position(axis.range.1));
    let (mut u0, mut u1) = state.window.unwrap_or(full);
    if let Some(hover) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        if scroll != 0.0 && plot.contains(hover) {
            let f = (-scroll as f64 * 0.002).exp();
            let at = u0 + (u1 - u0) * ((hover.x - plot.left()) / plot.width()) as f64;
            u0 = at - (at - u0) * f;
            u1 = at + (u1 - at) * f;
        }
    }
    if response.dragged() {
        let du = -(response.drag_delta().x / plot.width()) as f64 * (u1 - u0);
        u0 += du;
        u1 += du;
    }
    if response.double_clicked() {
        (u0, u1) = full;
    }
    let span = (u1 - u0).clamp((full.1 - full.0).abs() * 1e-9, full.1 - full.0);
    u0 = u0.clamp(full.0, full.1 - span);
    u1 = u0 + span;
    state.window = (u0 > full.0 || u1 < full.1).then_some((u0, u1));

    // Value range of what's visible.
    let (mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY);
    for tr in traces {
        for (x, y) in tr.x.iter().zip(tr.y) {
            let u = axis.position(*x);
            if u >= u0 && u <= u1 && y.is_finite() {
                v0 = v0.min(*y);
                v1 = v1.max(*y);
            }
        }
    }
    if !v0.is_finite() {
        (v0, v1) = (0.0, 1.0);
    }
    let pad = ((v1 - v0) * 0.08).max(if y_unit == "V" { 0.05 } else { 1.0 });
    (v0, v1) = (v0 - pad, v1 + pad);

    let px = |u: f64| plot.left() + ((u - u0) / (u1 - u0)) as f32 * plot.width();
    let py = |v: f64| plot.bottom() - ((v - v0) / (v1 - v0)) as f32 * plot.height();

    // Grid and axis labels.
    let font = FontId::monospace(10.5);
    let grid = Stroke::new(1.0, dim.gamma_multiply(0.45));
    let faint = Stroke::new(1.0, dim.gamma_multiply(0.2));
    let text_color = fg.gamma_multiply(0.7);
    let vline = |x: f32, s: Stroke| {
        painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], s)
    };
    let xlabel = |x: f32, text: String| {
        if !x_labels {
            return;
        }
        painter.text(
            Pos2::new(x, plot.bottom() + 4.0),
            Align2::CENTER_TOP,
            text,
            font.clone(),
            text_color,
        );
    };
    if axis.log {
        for decade in (u0.floor() as i32)..=(u1.ceil() as i32) {
            let base = decade as f64;
            if base >= u0 && base <= u1 {
                vline(px(base), grid);
                xlabel(px(base), label(10f64.powi(decade), axis.unit));
            }
            for m in 2..10 {
                let u = base + (m as f64).log10();
                if u > u0 && u < u1 {
                    vline(px(u), faint);
                }
            }
        }
    } else {
        for u in ticks(u0, u1, (plot.width() / 110.0) as f64) {
            vline(px(u), grid);
            xlabel(px(u), label(u, axis.unit));
        }
    }
    let count = (plot.height() / 40.0).max(1.5) as f64;
    let yticks =
        if y_unit == "°" { degree_ticks(v0, v1, count.max(3.0)) } else { ticks(v0, v1, count) };
    for v in yticks {
        let y = py(v);
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], grid);
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            label(v, y_unit),
            font.clone(),
            text_color,
        );
    }
    painter.rect_stroke(plot, 0.0, Stroke::new(1.0, dim), egui::StrokeKind::Inside);

    // Traces, reduced to a min/max pair per pixel column when dense.
    let clip = painter.with_clip_rect(plot);
    for (k, tr) in traces.iter().enumerate() {
        let (lo, hi) = (axis.value_at(u0), axis.value_at(u1));
        let first = tr.x.partition_point(|&x| x < lo).saturating_sub(1);
        let last = (tr.x.partition_point(|&x| x <= hi) + 1).min(tr.x.len());
        let visible = last.saturating_sub(first);
        let width = plot.width() as usize;
        let mut points: Vec<Pos2> = Vec::with_capacity(visible.min(4 * width));
        if visible > 4 * width {
            let mut column = f32::NAN;
            let (mut a, mut b) = (f64::INFINITY, f64::NEG_INFINITY);
            for i in first..last {
                let x = px(axis.position(tr.x[i])).round();
                if x != column && column.is_finite() && a.is_finite() {
                    points.push(Pos2::new(column, py(a)));
                    points.push(Pos2::new(column, py(b)));
                    (a, b) = (f64::INFINITY, f64::NEG_INFINITY);
                }
                column = x;
                if tr.y[i].is_finite() {
                    a = a.min(tr.y[i]);
                    b = b.max(tr.y[i]);
                }
            }
            if column.is_finite() && a.is_finite() {
                points.push(Pos2::new(column, py(a)));
                points.push(Pos2::new(column, py(b)));
            }
        } else {
            // Points that aren't finite (no signal: −∞ dB) break the line.
            for i in first..last {
                if tr.y[i].is_finite() {
                    points.push(Pos2::new(px(axis.position(tr.x[i])), py(tr.y[i])));
                } else if points.len() > 1 {
                    clip.add(egui::Shape::line(
                        std::mem::take(&mut points),
                        Stroke::new(1.6, trace_color(k + first_color)),
                    ));
                } else {
                    points.clear();
                }
            }
        }
        if points.len() > 1 {
            clip.add(egui::Shape::line(points, Stroke::new(1.6, trace_color(k + first_color))));
        }
    }

    // Cursor readout at the pointer's x, on every plot stacked in this panel.
    let pointer = ui.ctx().pointer_hover_pos();
    let cursor = pointer
        .filter(|p| p.x >= plot.left() && p.x <= plot.right() && ui.clip_rect().contains(*p));
    let at =
        cursor.map(|p| axis.value_at(u0 + (u1 - u0) * ((p.x - plot.left()) / plot.width()) as f64));
    if let (Some(p), Some(x)) = (cursor, at) {
        vline(p.x, Stroke::new(1.0, fg.gamma_multiply(0.5)));
        painter.text(
            Pos2::new(p.x + 4.0, plot.top() + 2.0),
            Align2::LEFT_TOP,
            label(x, axis.unit),
            font.clone(),
            fg,
        );
    }

    // Legend, with values at the cursor.
    let mut row = Pos2::new(plot.right() - 8.0, plot.top() + 6.0);
    for (k, tr) in traces.iter().enumerate() {
        let value = at.filter(|_| tr.x.len() >= 2).map(|x| {
            let i = tr.x.partition_point(|&t| t < x).clamp(1, tr.x.len() - 1);
            let (ua, ub) = (axis.position(tr.x[i - 1]), axis.position(tr.x[i]));
            let f = if ub > ua { (axis.position(x) - ua) / (ub - ua) } else { 1.0 };
            tr.y[i - 1] + f * (tr.y[i] - tr.y[i - 1])
        });
        let text = match value {
            Some(v) => format!("{}  {}", tr.name, label(v, y_unit)),
            None => tr.name.to_owned(),
        };
        let galley = painter.layout_no_wrap(text, font.clone(), trace_color(k + first_color));
        let r = Rect::from_min_size(row - Vec2::new(galley.size().x, 0.0), galley.size());
        painter.rect_filled(r.expand(2.0), 2.0, color(theme.background).gamma_multiply(0.85));
        painter.galley(r.min, galley, trace_color(k + first_color));
        row.y += 15.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round_numbers() {
        assert_eq!(ticks(0.0, 10.0, 5.0), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        let t = ticks(0.0, 3e-8, 6.0);
        assert_eq!(t.len(), 7);
        assert!((t[1] - 5e-9).abs() < 1e-20);
        assert!(ticks(1.0, 1.0, 5.0).is_empty());
    }

    #[test]
    fn phase_is_unwrapped_and_ticked_in_round_angles() {
        let mut p = [170.0, 179.0, -178.0, f64::NAN, -170.0];
        unwrap_phase(&mut p);
        assert_eq!([p[0], p[1], p[2], p[4]], [170.0, 179.0, 182.0, 190.0]);
        assert_eq!(degree_ticks(-200.0, 10.0, 4.0), [-180.0, -90.0, 0.0]);
    }

    #[test]
    fn decibels_and_degrees_skip_si_prefixes() {
        assert_eq!(label(-3.0, "dB"), "-3dB");
        assert_eq!(label(-45.25, "°"), "-45.2°");
        assert_eq!(label(1e-3, "s"), "1ms");
    }
}
