//! The right side (properties, simulation, physics), the results along the bottom,
//! and the parts palette floating over the sheet.

use super::*;
use crate::style::{self, icon};

impl CircuiterApp {
    pub(super) fn right_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = style::strip(ui, |ui| {
            style::tab_strip(ui, &["Properties", "Simulation", "Physics"], self.right_tab)
        }) {
            self.right_tab = t;
        }
        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            match self.right_tab {
                1 => self.simulation_panel(ui),
                2 => self.physics_panel(ui),
                _ => self.inspector(ui),
            }
        });
    }

    /// Running analyses and what they found.
    fn simulation_panel(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let mut simulate = None;
        let mut transient = false;
        let mut ac = false;
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let (tstop, _) = ws.default_tran();
            let running = ws.sim.as_ref().is_some_and(|s| s.job.is_some());
            style::section(ui, "Run", |ui| {
                ui.label(
                    RichText::new(format!(
                        "Run switches {} on and watches it for {}: waveforms below, voltages \
                         on the wires, and parts that overheat marked.",
                        ws.sim_top(),
                        show(tstop, "s")
                    ))
                    .color(p.weak),
                );
                ui.add_space(4.0);
                let run = egui::Button::new(
                    RichText::new(format!("{}  Run Simulation", icon::PLAY)).color(p.go).strong(),
                )
                .fill(p.go_soft)
                .stroke(egui::Stroke::new(1.0, p.go.gamma_multiply(0.6)))
                .min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add_enabled(!running, run).on_hover_text("Ctrl+R").clicked() {
                    simulate = Some(ws.run_analysis());
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.button("Operating Point").on_hover_text("F9").clicked() {
                        simulate = Some(Analysis::Op);
                    }
                    if ui.button("Transient…").on_hover_text("F10").clicked() {
                        transient = true;
                    }
                    if ui.button("AC Sweep…").clicked() {
                        ac = true;
                    }
                });
            });
            style::section(ui, "Result", |ui| match &ws.sim {
                None => {
                    ui.label(RichText::new("Nothing simulated yet.").color(p.weak));
                }
                Some(sim) => {
                    let (text, color) = sim_status(sim, p);
                    ui.label(RichText::new(text).color(color));
                    if let Some(e) = sim.error.as_ref().filter(|e| *e != "cancelled") {
                        ui.label(RichText::new(e).color(p.error));
                    }
                    for e in sim.events() {
                        ui.label(
                            RichText::new(format!("{}  {}", icon::FIRE, e.describe()))
                                .color(p.error),
                        );
                    }
                }
            });
            if let Some(sim) = &ws.sim {
                style::section(ui, "Plotted signals", |ui| {
                    if sim.probes.is_empty() {
                        ui.label(
                            RichText::new("Select a wire, then Add to Plot in Properties.")
                                .color(p.weak),
                        );
                    }
                    for (k, &probe) in sim.probes.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().rect_filled(dot, 2.0, plot::trace_color(k));
                            ui.label(&sim.netlist.nets[probe].name);
                            style::trailing(ui, |ui| {
                                let x = egui::Button::new(RichText::new(icon::X).color(p.weak))
                                    .frame(false);
                                if ui.add(x).on_hover_text("Remove from the plot").clicked() {
                                    remove = Some(k);
                                }
                            });
                        });
                    }
                });
            }
            if let Some(e) = &ws.sim_error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(p.error));
            }
        });
        if let (Some(k), Some(sim)) = (remove, &mut ws.sim) {
            sim.probes.remove(k);
        }
        if ac {
            self.prompt = Some(ac_prompt());
        }
        if transient {
            self.transient_prompt();
        }
        if let Some(a) = simulate {
            self.show_bottom = true;
            self.simulate(a);
        }
    }

    /// How hot parts run, what they're rated for, and what failed.
    fn physics_panel(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let selected = match ws.selection.iter().next() {
            Some(Owner::Instance(i)) if ws.selection.len() == 1 => Some(*i),
            _ => None,
        };
        let mut select = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let Some(i) = selected else {
                parts_table(ui, ws, p, &mut select);
                return;
            };
            let Some(inst) = ws.schematic().and_then(|s| s.instances.get(i)).cloned() else {
                return;
            };
            let lib = ws.current().cell.library.clone();
            let def = ws.design.resolve(&inst.cell, &lib).map(|(_, d)| d.clone());
            ui.label(RichText::new(&inst.name).size(20.0).strong());
            ui.label(RichText::new(inst.cell.to_string()).color(p.weak));
            let path = match ws.sheet_path() {
                s if s.is_empty() => inst.name.clone(),
                s => format!("{s}.{}", inst.name),
            };
            if let Some(physics) = def.as_ref().and_then(|d| d.cell.physics.as_ref()) {
                style::section(ui, "Ratings", |ui| {
                    style::properties(ui, "ratings", |ui| {
                        let row =
                            |ui: &mut egui::Ui, name: &str, v: &Option<String>, unit: &str| {
                                if let Some(v) = v {
                                    style::name(ui, name);
                                    let shown =
                                        rated_value(&inst, def.as_ref().expect("has physics"), v);
                                    style::value(ui, &format!("{shown} {unit}"), 150.0);
                                    ui.end_row();
                                }
                            };
                        row(ui, "Destroyed at", &physics.max_temperature, "°C");
                        row(ui, "Thermal resistance", &physics.thermal_resistance, "°C/W");
                        row(ui, "Heat capacity", &physics.heat_capacity, "J/°C");
                        for limit in &physics.limits {
                            let d = def.as_ref().expect("has physics");
                            let (name, v) = match limit {
                                circuiter_model::cell::Limit::Voltage { ports, max } => (
                                    format!("V({}, {}) max", ports.0, ports.1),
                                    format!("{} V", rated_value(&inst, d, max)),
                                ),
                                circuiter_model::cell::Limit::Current { port, max } => (
                                    format!("I({port}) max"),
                                    format!("{} A", rated_value(&inst, d, max)),
                                ),
                                circuiter_model::cell::Limit::Energy { port, max, .. } => (
                                    format!("I²t({port}) max"),
                                    format!("{} A²s", rated_value(&inst, d, max)),
                                ),
                            };
                            style::name(ui, &name);
                            style::value(ui, &v, 150.0);
                            ui.end_row();
                        }
                        style::name(ui, "When destroyed");
                        let mode = match &physics.failure.mode {
                            circuiter_model::cell::FailureMode::Open => "goes open".to_owned(),
                            circuiter_model::cell::FailureMode::Short(a, b) => {
                                format!("shorts {a}–{b}")
                            }
                        };
                        ui.label(format!("{mode}, {:?}", physics.failure.effect).to_lowercase());
                        ui.end_row();
                    });
                });
            } else {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("This part has no ratings: it can't overheat.").color(p.weak),
                );
            }
            let Some(sim) = ws.fresh_sim() else {
                ui.add_space(8.0);
                ui.label(RichText::new("Run a simulation to see how hot it gets.").color(p.weak));
                return;
            };
            if let Some(part) = sim.monitor.as_ref().and_then(|m| m.part(&path)) {
                style::section(ui, "In the last run", |ui| {
                    style::properties(ui, "heat", |ui| {
                        style::name(ui, "Temperature");
                        style::value(ui, &format!("{:.1} °C", part.temperature), 150.0);
                        ui.end_row();
                        style::name(ui, "Peak");
                        style::value(ui, &format!("{:.1} °C", part.peak), 150.0);
                        ui.end_row();
                        style::name(ui, if part.failed { "Power when it failed" } else { "Power" });
                        style::value(ui, &show(part.power, "W"), 150.0);
                        ui.end_row();
                    });
                    if let Some(max) = part.max_temperature() {
                        let used = ((part.peak - 25.0) / (max - 25.0)).clamp(0.0, 1.0) as f32;
                        ui.add_space(4.0);
                        let color = if used > 0.8 {
                            p.error
                        } else if used > 0.5 {
                            p.warning
                        } else {
                            p.go
                        };
                        ui.add(
                            egui::ProgressBar::new(used)
                                .fill(color)
                                .text(format!("{:.0}% of the way to destruction", used * 100.0)),
                        );
                    }
                });
            }
            if let Some(e) = sim.failure(&path) {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{}  {}", icon::FIRE, e.describe())).color(p.error));
            }
            if let Some(a) = sim.assessments.iter().find(|a| a.path == path) {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{}  Would fail: {}", icon::WARNING, a.reason))
                        .color(p.warning),
                );
            }
        });
        if let Some(i) = select {
            ws.select_only(Some(Owner::Instance(i)));
        }
    }

    pub(super) fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let problems = self.workspace.as_ref().map_or(0, |ws| ws.problems().len());
        let problems_tab = format!("Problems ({problems})");
        let mut hide = false;
        if let Some(t) = style::strip(ui, |ui| {
            ui.horizontal(|ui| {
                let t =
                    style::tab_strip(ui, &["Waveforms", "Thermal", &problems_tab], self.bottom_tab);
                style::trailing(ui, |ui| {
                    hide = style::icon_button(ui, icon::X, "Hide the results panel", true, false)
                        .clicked();
                });
                t
            })
            .inner
        }) {
            self.bottom_tab = t;
        }
        if hide {
            self.show_bottom = false;
            return;
        }
        let Some(ws) = &self.workspace else { return };
        let has_results = ws.sim.as_ref().is_some_and(|s| {
            matches!(s.results, Some(Results::Tran { .. } | Results::Ac { .. } | Results::Logic(_)))
        });
        match self.bottom_tab {
            2 => {
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(
                        ui,
                        |ui| {
                            let list = ws.problems();
                            if list.is_empty() {
                                ui.label(
                                    RichText::new(format!("{}  No problems", icon::CHECK_CIRCLE))
                                        .color(p.go),
                                );
                            }
                            for (severity, text) in list {
                                let (glyph, color) = match severity {
                                    Severity::Error => (icon::X_CIRCLE, p.error),
                                    Severity::Warning => (icon::WARNING, p.warning),
                                };
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(glyph).color(color));
                                    ui.label(text);
                                });
                            }
                        },
                    );
                });
            }
            tab if has_results => {
                self.plot_heat = tab == 1;
                egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    self.results_panel(ui);
                });
            }
            _ => {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.35).max(8.0));
                    ui.label(
                        RichText::new(format!("{}  No results yet", icon::WAVEFORM))
                            .size(15.0)
                            .color(p.weak),
                    );
                    ui.label(
                        RichText::new(
                            "Run Simulation (Ctrl+R): waveforms and temperatures appear here.",
                        )
                        .color(p.weak),
                    );
                });
            }
        }
    }

    /// The most-used parts, in a column beside the sheet: click one to place it, or
    /// drag it onto the sheet.
    pub(super) fn parts_column(&mut self, ui: &mut egui::Ui) {
        let ids: Vec<CellId> = PALETTE
            .iter()
            .map(|(name, _)| CellId::new(circuiter_model::STD_LIBRARY, name))
            .collect();
        self.cache_icons(&ids);
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let placing = match &ws.tool {
            Tool::Place { cell, .. } => ws.design.resolve(cell, &ws.current().cell.library),
            _ => None,
        }
        .map(|(id, _)| id);
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("parts-column").show(ui, |ui| {
            ui.add_space(6.0);
            egui::Grid::new("palette-grid").spacing([2.0, 2.0]).show(ui, |ui| {
                for (k, (id, (_, key))) in ids.iter().zip(PALETTE).enumerate() {
                    let (rect, r) = ui
                        .allocate_exact_size(egui::vec2(36.0, 30.0), egui::Sense::click_and_drag());
                    let active = placing.as_ref() == Some(id);
                    if active || r.hovered() {
                        let fill = if active {
                            p.accent_soft
                        } else {
                            ui.visuals().widgets.hovered.weak_bg_fill
                        };
                        ui.painter().rect_filled(rect, 5.0, fill);
                    }
                    if let Some(scene) = self.icons.get(id) {
                        canvas::draw_icon(ui.painter(), rect.shrink(6.0), scene, p.text);
                    }
                    let desc =
                        ws.design.cell(id).map(|d| d.cell.description.clone()).unwrap_or_default();
                    let key = if key.is_empty() { String::new() } else { format!("  ({key})") };
                    let r = r.on_hover_text(format!(
                        "{}{key}\n{desc}\nClick, then click the sheet · or drag it on",
                        id.cell
                    ));
                    let label = format!("Place {}", id.cell);
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                    });
                    r.dnd_set_drag_payload(DraggedCell(id.clone()));
                    if r.clicked() {
                        pick = Some(id.clone());
                    }
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        });
        if let Some(id) = pick {
            ws.start_placing(&id);
        }
    }
}

/// A rating as entered, with the part's parameters filled in.
fn rated_value(
    inst: &circuiter_model::schematic::Instance,
    def: &circuiter_model::CellDef,
    raw: &str,
) -> String {
    let mut out = raw.to_owned();
    for p in &def.cell.params {
        let placeholder = format!("${{param.{}}}", p.name);
        if out.contains(&placeholder) {
            let value =
                inst.params.get(&p.name).or(p.default.as_ref()).cloned().unwrap_or_default();
            out = out.replace(&placeholder, &value);
        }
    }
    out
}

/// Every part the last run heated, hottest first.
fn parts_table(ui: &mut egui::Ui, ws: &Workspace, p: &style::Palette, select: &mut Option<usize>) {
    let Some(monitor) = ws.fresh_sim().and_then(|s| s.monitor.as_ref()) else {
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Run a simulation to see how hot each part gets. Select a part for its \
                 ratings.",
            )
            .color(p.weak),
        );
        return;
    };
    let mut parts: Vec<_> = monitor.parts.iter().collect();
    parts.sort_by(|a, b| b.peak.total_cmp(&a.peak));
    style::section(ui, "Parts by peak temperature", |ui| {
        egui::Grid::new("parts").num_columns(4).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
            for h in ["Part", "Peak", "Power", ""] {
                ui.label(RichText::new(h).small().color(p.weak));
            }
            ui.end_row();
            let sheet = ws.sheet_path();
            for part in parts {
                let local = if sheet.is_empty() {
                    Some(part.path.as_str())
                } else {
                    part.path.strip_prefix(&format!("{sheet}.")).filter(|r| !r.contains('.'))
                };
                let index = local
                    .and_then(|name| ws.schematic()?.instances.iter().position(|i| i.name == name));
                if ui.link(&part.path).clicked() {
                    *select = index;
                }
                let hot = part.max_temperature().map_or(0.0, |m| (part.peak - 25.0) / (m - 25.0));
                let color = if part.failed || hot > 0.8 {
                    p.error
                } else if hot > 0.5 {
                    p.warning
                } else {
                    p.text
                };
                ui.label(RichText::new(format!("{:.1} °C", part.peak)).monospace().color(color));
                ui.label(RichText::new(show(part.power, "W")).monospace());
                if part.failed {
                    ui.label(RichText::new(icon::FIRE).color(p.error)).on_hover_text("Destroyed");
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
    });
}

/// A one-line summary of a simulation's state, and its colour.
pub(super) fn sim_status(
    sim: &crate::simulation::Simulation,
    p: &style::Palette,
) -> (String, Color32) {
    match (&sim.results, &sim.error, sim.stale) {
        _ if sim.job.is_some() => {
            let pct = sim.job.as_ref().map_or(0.0, |j| j.control.progress()) * 100.0;
            (format!("Simulating… {pct:.0}%  (Esc cancels)"), p.accent)
        }
        (_, Some(e), _) if e == "cancelled" => ("Cancelled".to_owned(), p.warning),
        (_, Some(_), _) => ("Simulation failed".to_owned(), p.error),
        (Some(_), None, true) => ("Results out of date: run again".to_owned(), p.warning),
        (Some(Results::Ac { result, .. }), None, false) => {
            (format!("AC sweep · {} frequencies", result.frequency.len()), p.go)
        }
        (Some(Results::Op(_)), None, false) => ("Operating point".to_owned(), p.go),
        (Some(Results::Tran { result, .. }), None, false) => {
            (format!("Transient · {} points", result.time.len()), p.go)
        }
        (Some(Results::Logic(run)), None, false) if run.halted => {
            (format!("Digital · halted at {} (the program ended)", show(run.end, "s")), p.go)
        }
        (Some(Results::Logic(run)), None, false) => {
            (format!("Digital · {} simulated", show(run.end, "s")), p.go)
        }
        (None, None, _) => (String::new(), p.weak),
    }
}
