//! The bench: running a circuit live and the instruments to watch it with. Switches
//! and buttons operated on the sheet, knobs in Properties, live waveforms, and the
//! oscilloscope.

use super::*;
use crate::live::{Command, ControlKind, Reading, Scope};
use crate::style::{self, icon};

/// One oscilloscope's front panel.
#[derive(Debug, Clone)]
pub struct ScopeSettings {
    pub on: [bool; 4],
    /// Volts per division, per channel.
    pub volts: [f64; 4],
    /// Seconds per division.
    pub time: f64,
    pub trigger_channel: usize,
    pub trigger_level: f64,
    pub rising: bool,
    /// Draw the latest data even without a trigger.
    pub auto: bool,
    /// Frozen: keeps showing what was on screen.
    pub stopped: bool,
    /// What was on screen when stopped: per channel, (time, volts).
    pub frozen: Option<Vec<Vec<(f64, f64)>>>,
    /// Scaled to its signals once, when they first had enough data.
    pub fitted: bool,
}

impl Default for ScopeSettings {
    fn default() -> Self {
        ScopeSettings {
            on: [true; 4],
            volts: [1.0; 4],
            time: 1e-3,
            trigger_channel: 0,
            trigger_level: 0.0,
            rising: true,
            auto: true,
            stopped: false,
            frozen: None,
            fitted: false,
        }
    }
}

/// Classic channel colours: yellow, cyan, magenta, blue.
const CHANNEL: [Color32; 4] = [
    Color32::from_rgb(240, 214, 60),
    Color32::from_rgb(70, 214, 230),
    Color32::from_rgb(226, 98, 204),
    Color32::from_rgb(80, 140, 255),
];

/// 1, 2 or 5 times a power of ten, at or above `x`.
fn nice(x: f64) -> f64 {
    if !(x.is_finite() && x > 0.0) {
        return 1.0;
    }
    let mag = 10f64.powf(x.log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|v| *v >= x * 0.999).unwrap_or(x)
}

/// The next step of a 1-2-5 sequence, up (`dir` 1) or down (`dir` -1).
fn step125(x: f64, dir: i32) -> f64 {
    let mag = 10f64.powf(x.log10().floor());
    let m = (x / mag).round();
    let steps = [1.0, 2.0, 5.0];
    let k = steps.iter().position(|s| (*s - m).abs() < 0.5).unwrap_or(0) as i32 + dir;
    match k {
        -1 => 5.0 * mag / 10.0,
        3 => 10.0 * mag,
        k => steps[k as usize] * mag,
    }
}

impl CircuiterApp {
    /// Per frame while running live: follow the circuit, keep the window repainting,
    /// and stop on errors.
    pub(super) fn live_frame(&mut self, ctx: &egui::Context) {
        let Some(ws) = &mut self.workspace else { return };
        let Some(live) = &ws.live else { return };
        let error = live.shared.lock().ok().and_then(|s| s.error.clone());
        if let Some(e) = error {
            self.error = Some(format!("live run stopped: {e}"));
            ws.stop_live();
            return;
        }
        ws.follow_live();
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }

    /// Clicks and presses on the sheet that operate parts while running live: they
    /// don't select or move anything.
    pub(super) fn live_interact(
        &mut self,
        ctx: &egui::Context,
        gestures: Vec<canvas::Gesture>,
    ) -> Vec<canvas::Gesture> {
        let Some(ws) = &self.workspace else { return gestures };
        let Some(live) = &ws.live else { return gestures };
        let hovered = match self.hovered {
            Some(Owner::Instance(i)) => {
                ws.instance_path(i).and_then(|p| Some((p.clone(), live.control(&p)?)))
            }
            _ => None,
        };
        let (pressed, released) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        if pressed {
            if let Some((path, kind)) = &hovered {
                self.live_press = true;
                if kind.momentary() {
                    live.send(Command::Set(path.clone(), "true".into()));
                    self.live_held = Some(path.clone());
                }
            }
        }
        if released {
            if let Some(path) = self.live_held.take() {
                live.send(Command::Set(path, "false".into()));
            }
        }
        let mut out = Vec::new();
        for g in gestures {
            match g {
                canvas::Gesture::Click { .. } if hovered.is_some() => {
                    let (path, kind) = hovered.clone().expect("checked");
                    if kind.clicks() {
                        let states = ws.live_states();
                        let on = states.get(&path).is_some_and(|v| v == "true");
                        live.send(Command::Set(path, (!on).to_string()));
                    }
                }
                canvas::Gesture::DragStart { .. } | canvas::Gesture::Drag { .. }
                    if self.live_press => {}
                canvas::Gesture::DragEnd { .. } if self.live_press => {}
                other => out.push(other),
            }
        }
        if released {
            self.live_press = false;
        }
        out
    }

    /// The live waveforms: the watched nets over the last stretch of time.
    pub(super) fn live_waveforms(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        let Some(ws) = &self.workspace else { return };
        let Some(live) = &ws.live else { return };
        let Ok(shared) = live.shared.lock() else { return };
        let (t, window) = (shared.time, shared.window);
        let lo = (t - window).max(0.0);
        let mut analog: Vec<(String, Vec<f64>, Vec<f64>)> = Vec::new();
        type Bits = (String, Vec<(u64, circuiter_logic::Value)>, u32);
        let mut logic: Vec<Bits> = Vec::new();
        for &net in &ws.live_watch {
            let name = live.netlist.nets[net].name.clone();
            let Some(tr) = shared.traces.get(&net) else { continue };
            if !tr.analog.is_empty() {
                let (x, y): (Vec<f64>, Vec<f64>) = tr.analog.iter().copied().unzip();
                analog.push((name, x, y));
            } else if !tr.logic.is_empty() {
                logic.push((name, tr.logic.clone(), live.netlist.nets[net].width));
            }
        }
        drop(shared);
        let traces: Vec<Trace<'_>> =
            analog.iter().map(|(n, x, y)| Trace { name: n, x, y }).collect();
        let bits: Vec<plot::LogicTrace<'_>> = logic
            .iter()
            .map(|(n, c, w)| plot::LogicTrace { name: n, changes: c, width: *w })
            .collect();
        let share = match (traces.is_empty(), bits.is_empty()) {
            (false, true) => 1.0,
            (true, false) => 0.0,
            (true, true) => {
                ui.label(RichText::new("Select a wire and choose Add to Plot to watch it").weak());
                return;
            }
            _ => traces.len() as f32 / (traces.len() + bits.len()) as f32,
        };
        let (top, bottom) = area.split_top_bottom_at_fraction(share);
        let mut state = PlotState::default();
        if !traces.is_empty() {
            let axis = XAxis { unit: "s", range: (lo, t.max(lo + 1e-15)), log: false };
            plot::show_lanes(ui, top, &mut state, &traces, axis, "V", self.theme);
        }
        if !bits.is_empty() {
            plot::show_logic(ui, bottom, &mut state, &bits, t.max(1e-15), self.theme);
        }
    }

    /// The oscilloscope: a screen, its channels and timebase, trigger and measurements.
    pub(super) fn scope_view(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let Some(ws) = &self.workspace else { return };
        // Live, or the last run's results.
        let (scopes, live) = match &ws.live {
            Some(l) => (l.scopes.clone(), true),
            None => match ws.fresh_sim() {
                Some(sim) => (Scope::all(&sim.netlist), false),
                None => (Vec::new(), false),
            },
        };
        let Some(scope) = scopes.first().cloned() else {
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);
                ui.label(
                    RichText::new(format!(
                        "{}  No oscilloscope on the sheet. Place one (Instruments in Components), \
                         clip its probes onto the wires to watch, and run.",
                        icon::WAVE_SINE
                    ))
                    .color(p.weak),
                );
            });
            return;
        };
        // Each channel's samples, against COM.
        let samples = |net: Option<usize>| -> Vec<(f64, f64)> {
            let Some(net) = net else { return Vec::new() };
            if let Some(l) = &ws.live {
                let Ok(sh) = l.shared.lock() else { return Vec::new() };
                return sh
                    .traces
                    .get(&net)
                    .map(|t| t.analog.iter().copied().collect())
                    .unwrap_or_default();
            }
            let Some(sim) = ws.fresh_sim() else { return Vec::new() };
            match sim.waveform(net) {
                Some((t, v)) => t.iter().copied().zip(v).collect(),
                None => Vec::new(),
            }
        };
        let com = samples(scope.com);
        let mut data: Vec<Vec<(f64, f64)>> = scope
            .channels
            .iter()
            .map(|ch| {
                let mut s = samples(*ch);
                if !com.is_empty() {
                    for (t, v) in &mut s {
                        let k = com.partition_point(|(ct, _)| ct < t).min(com.len() - 1);
                        *v -= com[k].1;
                    }
                }
                s
            })
            .collect();
        let settings = self.scopes.entry(scope.path.clone()).or_default();
        if settings.stopped {
            match &settings.frozen {
                Some(f) => data = f.clone(),
                None => settings.frozen = Some(data.clone()),
            }
        } else {
            settings.frozen = None;
        }

        // The front panel, in a column at the right; the screen takes the rest.
        let full = ui.available_rect_before_wrap();
        let panel_w = 250.0f32.min(full.width() * 0.45);
        let panel =
            egui::Rect::from_min_max(egui::pos2(full.right() - panel_w, full.top()), full.max);
        let mut autoscale = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
            egui::ScrollArea::vertical().id_salt("scope-panel").show(ui, |ui| {
                ui.label(RichText::new(format!("{}  {}", icon::WAVE_SINE, scope.path)).strong());
                ui.add_space(4.0);
                egui::Grid::new("scope-channels").num_columns(4).spacing([6.0, 4.0]).show(
                    ui,
                    |ui| {
                        for (k, ch) in scope.channels.iter().enumerate() {
                            let label =
                                RichText::new(format!("CH{}", k + 1)).color(CHANNEL[k]).strong();
                            let connected = ch.is_some();
                            ui.add_enabled(
                                connected,
                                egui::Button::selectable(settings.on[k] && connected, label),
                            )
                            .on_hover_text(if connected {
                                "Show or hide"
                            } else {
                                "Its probe is on nothing"
                            })
                            .clicked()
                            .then(|| settings.on[k] = !settings.on[k]);
                            ui.label(
                                RichText::new(format!(
                                    "{}/div",
                                    crate::simulation::show(settings.volts[k], "V")
                                ))
                                .monospace(),
                            );
                            if ui.add_enabled(connected, egui::Button::new("−").small()).clicked()
                            {
                                settings.volts[k] = step125(settings.volts[k], -1);
                            }
                            if ui.add_enabled(connected, egui::Button::new("+").small()).clicked() {
                                settings.volts[k] = step125(settings.volts[k], 1);
                            }
                            ui.end_row();
                        }
                        ui.label(RichText::new("Time").strong());
                        ui.label(
                            RichText::new(format!(
                                "{}/div",
                                crate::simulation::show(settings.time, "s")
                            ))
                            .monospace(),
                        );
                        if ui.small_button("−").clicked() {
                            settings.time = step125(settings.time, -1);
                        }
                        if ui.small_button("+").clicked() {
                            settings.time = step125(settings.time, 1);
                        }
                        ui.end_row();
                    },
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Trigger").strong());
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("trigger-source")
                        .selected_text(format!("CH{}", settings.trigger_channel + 1))
                        .width(56.0)
                        .show_ui(ui, |ui| {
                            for k in 0..4 {
                                ui.selectable_value(
                                    &mut settings.trigger_channel,
                                    k,
                                    format!("CH{}", k + 1),
                                );
                            }
                        });
                    ui.add(
                        egui::DragValue::new(&mut settings.trigger_level)
                            .speed(0.01)
                            .custom_formatter(|v, _| crate::simulation::show(v, "V")),
                    )
                    .on_hover_text("Trigger level: drag");
                    let edge = if settings.rising { "↗" } else { "↘" };
                    if ui.button(edge).on_hover_text("Rising or falling edge").clicked() {
                        settings.rising = !settings.rising;
                    }
                });
                ui.checkbox(&mut settings.auto, "Auto (show without a trigger)");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let run = if settings.stopped {
                        format!("{}  Run", icon::PLAY)
                    } else {
                        format!("{}  Stop", icon::STOP)
                    };
                    if ui.button(run).on_hover_text("Freeze the screen").clicked() {
                        settings.stopped = !settings.stopped;
                    }
                    if ui.button("Autoscale").clicked() {
                        autoscale = true;
                    }
                });
            });
        });
        // Fit the signals the first time there's a stretch of them to fit.
        let enough = data.iter().any(|d| d.len() > 50 && d[d.len() - 1].0 - d[0].0 > 0.0);
        if !settings.fitted && enough {
            settings.fitted = true;
            autoscale = true;
        }
        if autoscale {
            // Fit each channel into six divisions, and the slowest repeat into five.
            for (k, d) in data.iter().enumerate() {
                let (lo, hi) =
                    d.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| {
                        (a.min(*v), b.max(*v))
                    });
                if hi.is_finite() {
                    settings.volts[k] = nice((hi.abs().max(lo.abs()) / 3.0).max(1e-3));
                }
            }
            if let Some(f) = data.get(settings.trigger_channel).and_then(|d| frequency(d)) {
                settings.time = nice(2.0 / f / 10.0);
            } else if let Some(span) =
                data.iter().find(|d| d.len() > 1).map(|d| d[d.len() - 1].0 - d[0].0)
            {
                settings.time = nice(span / 10.0);
            }
            if let Some(d) = data.get(settings.trigger_channel) {
                let (lo, hi) =
                    d.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| {
                        (a.min(*v), b.max(*v))
                    });
                if hi.is_finite() {
                    settings.trigger_level = (lo + hi) / 2.0;
                }
            }
        }
        let settings = settings.clone();

        // The screen: ten divisions across, eight up, the trigger point in the middle.
        let area =
            egui::Rect::from_min_max(full.min, egui::pos2(panel.left() - 10.0, full.bottom()));
        let meas_h = 22.0;
        let screen =
            egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.max.y - meas_h));
        if screen.height() < 40.0 || screen.width() < 80.0 {
            return;
        }
        let side = (screen.width() / 10.0).min(screen.height() / 8.0);
        let w = side * 10.0;
        let h = side * 8.0;
        let screen = egui::Rect::from_center_size(screen.center(), egui::vec2(w, h));
        ui.allocate_rect(full, egui::Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(screen.expand(4.0), 6.0, Color32::from_rgb(8, 14, 12));
        let grid = Color32::from_rgb(40, 70, 58);
        for i in 0..=10 {
            let x = screen.left() + side * i as f32;
            painter.line_segment(
                [egui::pos2(x, screen.top()), egui::pos2(x, screen.bottom())],
                egui::Stroke::new(if i == 5 { 1.2 } else { 0.6 }, grid),
            );
        }
        for j in 0..=8 {
            let y = screen.top() + side * j as f32;
            painter.line_segment(
                [egui::pos2(screen.left(), y), egui::pos2(screen.right(), y)],
                egui::Stroke::new(if j == 4 { 1.2 } else { 0.6 }, grid),
            );
        }
        // Where to start: a trigger crossing with five divisions of data after it.
        let span = settings.time * 10.0;
        let latest = data.iter().filter_map(|d| d.last().map(|(t, _)| *t)).fold(0.0, f64::max);
        let trig = data.get(settings.trigger_channel).and_then(|d| {
            let lvl = settings.trigger_level;
            d.windows(2).rev().find_map(|w| {
                let (a, b) = (w[0], w[1]);
                let crosses =
                    if settings.rising { a.1 < lvl && b.1 >= lvl } else { a.1 > lvl && b.1 <= lvl };
                let t = a.0 + (lvl - a.1) / (b.1 - a.1) * (b.0 - a.0);
                (crosses && t + span / 2.0 <= latest).then_some(t)
            })
        });
        let start = match (trig, settings.auto) {
            (Some(t), _) => Some(t - span / 2.0),
            (None, true) => Some(latest - span),
            (None, false) => None,
        };
        if let Some(t0) = start {
            for (k, d) in data.iter().enumerate() {
                if !settings.on[k] || scope.channels[k].is_none() || d.is_empty() {
                    continue;
                }
                let per = settings.volts[k];
                let first = d.partition_point(|(t, _)| *t < t0).saturating_sub(1);
                let pts: Vec<egui::Pos2> = d[first..]
                    .iter()
                    .take_while(|(t, _)| *t <= t0 + span * 1.001)
                    .map(|(t, v)| {
                        let x = screen.left() + ((t - t0) / span) as f32 * w;
                        let y = screen.center().y - (*v / per) as f32 * side;
                        egui::pos2(x, y.clamp(screen.top() - 2.0, screen.bottom() + 2.0))
                    })
                    .collect();
                painter
                    .with_clip_rect(screen)
                    .add(egui::Shape::line(pts, egui::Stroke::new(1.6, CHANNEL[k])));
                // The channel's ground level.
                painter.text(
                    egui::pos2(screen.left() - 2.0, screen.center().y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{}▶", k + 1),
                    egui::FontId::monospace(10.0),
                    CHANNEL[k],
                );
            }
            if trig.is_some() {
                let ty = screen.center().y
                    - (settings.trigger_level / settings.volts[settings.trigger_channel]) as f32
                        * side;
                painter.text(
                    egui::pos2(screen.right() + 2.0, ty),
                    egui::Align2::LEFT_CENTER,
                    "◀T",
                    egui::FontId::monospace(10.0),
                    CHANNEL[settings.trigger_channel],
                );
            }
        } else {
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for trigger",
                egui::FontId::proportional(14.0),
                grid,
            );
        }
        let status = match (live, trig.is_some(), settings.stopped) {
            (_, _, true) => "Stopped",
            (true, true, _) => "Trig'd",
            (true, false, _) => "Auto",
            (false, _, _) => "Last run",
        };
        painter.text(
            screen.right_top() + egui::vec2(-6.0, 6.0),
            egui::Align2::RIGHT_TOP,
            status,
            egui::FontId::proportional(12.0),
            Color32::from_rgb(140, 220, 160),
        );
        // Measurements.
        let mut text = Vec::new();
        for (k, d) in data.iter().enumerate() {
            if !settings.on[k] || scope.channels[k].is_none() || d.is_empty() {
                continue;
            }
            let (lo, hi) = d
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| (a.min(*v), b.max(*v)));
            let mean = d.iter().map(|(_, v)| v).sum::<f64>() / d.len() as f64;
            let f = frequency(d)
                .map(|f| format!("  f {}", crate::simulation::show(f, "Hz")))
                .unwrap_or_default();
            text.push((
                k,
                format!(
                    "CH{}  Vpp {}  mean {}{f}",
                    k + 1,
                    crate::simulation::show(hi - lo, "V"),
                    crate::simulation::show(mean, "V")
                ),
            ));
        }
        let mut x = area.left() + 8.0;
        for (k, t) in text {
            let r = painter.text(
                egui::pos2(x, area.bottom() - meas_h / 2.0),
                egui::Align2::LEFT_CENTER,
                t,
                egui::FontId::monospace(11.5),
                CHANNEL[k],
            );
            x = r.right() + 18.0;
        }
    }
}

/// The frequency of a repeating signal: from its crossings of its mean, going up.
fn frequency(d: &[(f64, f64)]) -> Option<f64> {
    if d.len() < 8 {
        return None;
    }
    let mean = d.iter().map(|(_, v)| v).sum::<f64>() / d.len() as f64;
    let (lo, hi) =
        d.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| (a.min(*v), b.max(*v)));
    if hi - lo < 1e-9 {
        return None;
    }
    // Hysteresis: a crossing counts after the signal has been well below the mean.
    let band = (hi - lo) * 0.1;
    let mut armed = false;
    let mut ups = Vec::new();
    for w in d.windows(2) {
        if w[0].1 < mean - band {
            armed = true;
        }
        if armed && w[0].1 < mean && w[1].1 >= mean {
            ups.push(w[0].0 + (mean - w[0].1) / (w[1].1 - w[0].1) * (w[1].0 - w[0].0));
            armed = false;
        }
    }
    (ups.len() >= 2).then(|| (ups.len() - 1) as f64 / (ups[ups.len() - 1] - ups[0]))
}

/// The live controls for a part in Properties: flip it, hold it, turn it.
pub(super) fn live_part_controls(ui: &mut egui::Ui, ws: &mut Workspace, index: usize) {
    let Some(live) = &ws.live else { return };
    let Some(path) = ws.instance_path(index) else { return };
    let Some(kind) = live.control(&path) else { return };
    let p = style::palette(ui);
    let states = ws.live_states();
    let value = states.get(&path).cloned().unwrap_or_default();
    section(ui, "Live");
    let mut set = None;
    match kind {
        ControlKind::Switch | ControlKind::Toggle | ControlKind::Changeover => {
            let on = value == "true";
            let word = match (kind, on) {
                (ControlKind::Switch, true) => "Closed",
                (ControlKind::Switch, false) => "Open",
                (ControlKind::Changeover, true) => "Thrown: com to b",
                (ControlKind::Changeover, false) => "At rest: com to a",
                (_, true) => "On (1)",
                (_, false) => "Off (0)",
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(word).strong().color(if on { p.go } else { p.weak }));
                if ui.button("Flip").on_hover_text("Or click the part on the sheet").clicked() {
                    set = Some((!on).to_string());
                }
            });
        }
        ControlKind::Pushbutton | ControlKind::Button => {
            let r = ui.add(
                egui::Button::new(format!("{}  Hold to press", icon::HAND))
                    .min_size(egui::vec2(160.0, 28.0)),
            );
            let down = r.is_pointer_button_down_on();
            if down != (value == "true") {
                set = Some(down.to_string());
            }
        }
        ControlKind::Dial => {
            let lookup = |name: &str, default: f64| -> f64 {
                ws.schematic()
                    .and_then(|s| s.instances.get(index))
                    .and_then(|i| i.params.get(name).cloned())
                    .and_then(|v| circuiter_model::units::parse_quantity(&v, None).ok())
                    .unwrap_or(default)
            };
            let (lo, hi) = (lookup("min", 0.0), lookup("max", 10.0));
            let mut v: f64 = circuiter_model::units::parse_quantity(&value, None).unwrap_or(lo);
            if ui.add(egui::Slider::new(&mut v, lo..=hi).suffix(" V")).changed() {
                set = Some(format!("{v}"));
            }
        }
        ControlKind::Potentiometer => {
            let mut v: f64 = value.parse().unwrap_or(0.5);
            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).text("position")).changed() {
                set = Some(format!("{v}"));
            }
        }
        ControlKind::Light | ControlKind::Sunlight | ControlKind::Temperature => {
            let mut v: f64 = circuiter_model::units::parse_quantity(&value, None).unwrap_or(0.0);
            let slider = match kind {
                // Moonlight to direct sun, on a logarithmic scale.
                ControlKind::Light => {
                    egui::Slider::new(&mut v, 0.1..=100_000.0).logarithmic(true).suffix(" lx")
                }
                ControlKind::Sunlight => egui::Slider::new(&mut v, 0.0..=1200.0).suffix(" W/m²"),
                _ => egui::Slider::new(&mut v, -40.0..=150.0).suffix(" °C"),
            };
            if ui.add(slider).changed() {
                set = Some(format!("{v}"));
            }
            let hint = match kind {
                ControlKind::Light => "Indoors a few hundred lux, daylight tens of thousands",
                ControlKind::Sunlight => "Full sun is about 1000 W/m²",
                _ => "Its temperature",
            };
            ui.label(RichText::new(hint).small().color(p.weak));
        }
    }
    if let Some(v) = set {
        live.send(Command::Set(path, v));
    }
}

/// A multimeter's reading in Properties, large, as on its display.
pub(super) fn live_meter_display(ui: &mut egui::Ui, reading: Option<Reading>) {
    let p = style::palette(ui);
    section(ui, "Reading");
    egui::Frame::new()
        .fill(Color32::from_rgb(20, 32, 26))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            let text = reading.map_or("— — —".to_owned(), Reading::show);
            ui.label(
                RichText::new(text).monospace().size(26.0).color(Color32::from_rgb(150, 240, 170)),
            );
        });
    ui.label(RichText::new("Run the circuit (or run it live) to measure.").small().color(p.weak));
}
