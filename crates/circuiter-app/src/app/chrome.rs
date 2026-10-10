//! The frame around the sheet: title bar, toolbar, document tabs, the bar above the
//! canvas with its view switcher and drawing tools, and the status bar.

use super::*;
use crate::style::{self, icon, icon_button, toolbar_gap};

/// Pixels per millimetre shown as 100 % zoom.
const ZOOM_100: f32 = 6.0;

impl CircuiterApp {
    /// Logo, menus and the search field.
    pub(super) fn title_bar(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        ui.horizontal(|ui| {
            ui.set_height(40.0);
            ui.add_space(8.0);
            style::logo(ui, 26.0);
            ui.label(RichText::new("Circuiter").size(19.0).strong().color(p.text));
            ui.add_space(18.0);
            self.menu_bar(ui);
            style::trailing(ui, |ui| {
                ui.add_space(10.0);
                let has_ws = self.workspace.is_some();
                let (rect, r) =
                    ui.allocate_exact_size(egui::vec2(300.0, 28.0), egui::Sense::click());
                let hovered = r.hovered() && has_ws;
                let stroke = if hovered { p.accent.gamma_multiply(0.6) } else { p.border };
                ui.painter().rect(
                    rect,
                    6.0,
                    p.well,
                    egui::Stroke::new(1.0, stroke),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.left_center() + egui::vec2(12.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{}   Search parts…", icon::MAGNIFYING_GLASS),
                    egui::FontId::proportional(13.0),
                    p.weak,
                );
                ui.painter().text(
                    rect.right_center() - egui::vec2(10.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    "Ctrl+K",
                    egui::FontId::proportional(11.5),
                    p.weak.gamma_multiply(0.8),
                );
                let r = r.on_hover_text("Find a part and place it");
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, has_ws, "Search parts")
                });
                if r.clicked() && has_ws {
                    self.prompt = Some(Prompt::AddPart { query: String::new() });
                }
            });
        });
    }

    /// File, edit and navigation buttons, and the simulation controls.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = style::palette(ui);
        let mut simulate = None;
        let mut transient = false;
        let mut prompt = None;
        let mut save = false;
        let mut error = None;
        ui.horizontal(|ui| {
            ui.set_height(40.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(8.0);
            let has_ws = self.workspace.is_some();
            if icon_button(ui, icon::FILE_PLUS, "New cell  (Ctrl+N)", has_ws, false).clicked() {
                prompt = self.workspace.as_ref().map(new_cell_prompt);
            }
            if icon_button(ui, icon::FOLDER_OPEN, "Open project  (Ctrl+O)", true, false).clicked() {
                self.guarded(&ctx, Pending::OpenDialog);
            }
            save =
                icon_button(ui, icon::FLOPPY_DISK, "Save  (Ctrl+S)", self.dirty(), false).clicked();
            toolbar_gap(ui);
            let Some(ws) = &mut self.workspace else { return };
            let editable = ws.editable();
            let sheet = ws.mode == Mode::Schematic;
            let has_sel = !ws.selection.is_empty();
            if icon_button(ui, icon::ARROW_ARC_LEFT, "Undo  (Ctrl+Z)", ws.can_undo(), false)
                .clicked()
            {
                ws.undo();
            }
            if icon_button(ui, icon::ARROW_ARC_RIGHT, "Redo  (Ctrl+Y)", ws.can_redo(), false)
                .clicked()
            {
                ws.redo();
            }
            toolbar_gap(ui);
            let placing = matches!(ws.tool, Tool::Place { .. } | Tool::Pin { .. });
            let turnable = editable && (has_sel || placing);
            if icon_button(ui, icon::ARROW_CLOCKWISE, "Rotate  (R)", turnable, false).clicked() {
                ws.rotate();
            }
            if icon_button(ui, icon::FLIP_HORIZONTAL, "Mirror  (X)", turnable, false).clicked() {
                ws.mirror();
            }
            if icon_button(ui, icon::TRASH, "Delete  (Del)", editable && has_sel, false).clicked() {
                ws.delete_selection();
            }
            let copyable = sheet && has_sel;
            if icon_button(ui, icon::SCISSORS, "Cut  (Ctrl+X)", editable && copyable, false)
                .clicked()
            {
                if let Some(text) = ws.cut() {
                    ctx.copy_text(text);
                }
            }
            if icon_button(ui, icon::COPY, "Copy  (Ctrl+C)", copyable, false).clicked() {
                if let Some(text) = ws.copy() {
                    ctx.copy_text(text);
                }
            }
            if icon_button(
                ui,
                icon::SQUARES_FOUR,
                "Duplicate  (Ctrl+D)",
                editable && copyable,
                false,
            )
            .clicked()
            {
                if let Err(e) = ws.duplicate() {
                    error = Some(e);
                }
            }
            toolbar_gap(ui);
            let back = ws.back_to().map(|c| format!("Back to {c}  (Alt+Left)"));
            let forward = ws.forward_to().map(|c| format!("Forward to {c}  (Alt+Right)"));
            if icon_button(
                ui,
                icon::ARROW_LEFT,
                back.as_deref().unwrap_or("Back"),
                back.is_some(),
                false,
            )
            .clicked()
            {
                ws.go_back();
            }
            if icon_button(
                ui,
                icon::ARROW_RIGHT,
                forward.as_deref().unwrap_or("Forward"),
                forward.is_some(),
                false,
            )
            .clicked()
            {
                ws.go_forward();
            }
            let nested = ws.levels.len() > 1;
            if icon_button(ui, icon::ARROW_LINE_UP, "Up one level  (Alt+Up)", nested, false)
                .clicked()
            {
                ws.ascend_to(ws.levels.len() - 1);
            }

            style::trailing(ui, |ui| {
                ui.add_space(8.0);
                ui.menu_button(RichText::new(icon::GEAR_SIX).size(17.0), |ui| {
                    ui.label(RichText::new("Theme").strong());
                    for theme in Theme::ALL {
                        if ui.radio(std::ptr::eq(self.theme, theme), theme.name).clicked() {
                            self.theme = theme;
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_grid, "Grid");
                    ui.checkbox(&mut self.show_bottom, "Results panel");
                })
                .response
                .on_hover_text("Settings");
                ui.add_space(6.0);
                let running = ws.sim.as_ref().is_some_and(|s| s.job.is_some());
                let (tstop, _) = ws.default_tran();
                let time = egui::Button::new(
                    RichText::new(format!("Time: {}  {}", show(tstop, "s"), icon::CARET_DOWN))
                        .color(p.text),
                )
                .min_size(egui::vec2(120.0, 28.0));
                if ui.add(time).on_hover_text("How long Run watches the circuit").clicked() {
                    transient = true;
                }
                ui.add_space(4.0);
                let stop = egui::Button::new(
                    RichText::new(format!("{}  Stop", icon::STOP)).color(if running {
                        p.error
                    } else {
                        p.weak
                    }),
                )
                .min_size(egui::vec2(80.0, 28.0));
                if ui.add_enabled(running, stop).on_hover_text("Esc").clicked() {
                    if let Some(sim) = &mut ws.sim {
                        sim.cancel();
                    }
                }
                ui.add_space(4.0);
                // Run, with the other analyses in its drop-down.
                ui.menu_button(RichText::new(icon::CARET_DOWN).color(p.go), |ui| {
                    if ui
                        .button("Operating Point   F9")
                        .on_hover_text("DC voltages, currents and power, as if on forever")
                        .clicked()
                    {
                        simulate = Some(Analysis::Op);
                        ui.close();
                    }
                    if ui
                        .button("Transient…   F10")
                        .on_hover_text("Waveforms over a time you choose")
                        .clicked()
                    {
                        transient = true;
                        ui.close();
                    }
                    if ui
                        .button("AC Sweep…")
                        .on_hover_text("Gain and phase over frequency")
                        .clicked()
                    {
                        prompt = Some(ac_prompt());
                        ui.close();
                    }
                });
                let label = if running {
                    let progress = ws.sim.as_ref().and_then(|s| s.job.as_ref());
                    let pct = progress.map_or(0.0, |j| j.control.progress()) * 100.0;
                    format!("{}  Simulating {pct:.0}%", icon::HOURGLASS)
                } else {
                    format!("{}  Run Simulation", icon::PLAY)
                };
                let run = egui::Button::new(RichText::new(label).color(p.go).strong())
                    .fill(p.go_soft)
                    .stroke(egui::Stroke::new(1.0, p.go.gamma_multiply(0.6)))
                    .min_size(egui::vec2(150.0, 28.0));
                let tip = format!(
                    "Ctrl+R: switch {} on and watch it for {}. Waveforms appear below, \
                     voltages on the wires, and parts that overheat are marked",
                    ws.sim_top(),
                    show(tstop, "s")
                );
                if ui.add_enabled(!running, run).on_hover_text(tip).clicked() {
                    simulate = Some(ws.run_analysis());
                }
            });
        });
        if prompt.is_some() {
            self.prompt = prompt;
        }
        if error.is_some() {
            self.error = error;
        }
        if save {
            self.save();
        }
        if let Some(a) = simulate {
            self.show_bottom = true;
            self.simulate(a);
        }
        if transient {
            self.transient_prompt();
        }
    }

    /// The open cells, one tab each.
    pub(super) fn doc_tabs(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let tabs = ws.tabs();
        let active = ws.active_tab();
        let mut switch = None;
        let mut close = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for (i, id) in tabs.iter().enumerate() {
                let on = i == active;
                let schematic = ws.design.cell(id).is_some_and(|d| d.schematic.is_some());
                let glyph = if schematic { icon::CIRCUITRY } else { icon::CPU };
                let name =
                    if id.library == ws.project.name() { id.cell.clone() } else { id.to_string() };
                let text = format!("{glyph}  {name}");
                let galley = egui::WidgetText::from(RichText::new(&text).color(if on {
                    p.text
                } else {
                    p.weak
                }))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Button,
                );
                let closable = tabs.len() > 1;
                let width = galley.size().x + if closable { 52.0 } else { 28.0 };
                let (rect, r) =
                    ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::click());
                let fill = if on {
                    p.panel
                } else if r.hovered() {
                    ui.visuals().widgets.hovered.weak_bg_fill
                } else {
                    p.raised
                };
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                    fill,
                );
                if on {
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0));
                    ui.painter().rect_filled(bar, 0.0, p.accent);
                }
                ui.painter().galley(
                    egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    p.text,
                );
                let r = r.on_hover_text(id.to_string());
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                if closable {
                    let x = egui::Rect::from_center_size(
                        egui::pos2(rect.right() - 16.0, rect.center().y),
                        egui::vec2(18.0, 18.0),
                    );
                    let xr = ui.interact(x, ui.id().with(("close-tab", i)), egui::Sense::click());
                    if xr.hovered() {
                        ui.painter().rect_filled(x, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
                    }
                    if on || r.hovered() || xr.hovered() {
                        ui.painter().text(
                            x.center(),
                            egui::Align2::CENTER_CENTER,
                            icon::X,
                            egui::FontId::proportional(12.0),
                            p.weak,
                        );
                    }
                    let xr = xr.on_hover_text("Close");
                    xr.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Close {name}"),
                        )
                    });
                    if xr.clicked() {
                        close = Some(i);
                    }
                }
                if r.clicked() {
                    switch = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
            }
        });
        if let Some(i) = close {
            ws.close_tab(i);
        } else if let Some(i) = switch {
            ws.switch_tab(i);
        }
    }

    /// Above the canvas: which view of the cell, where in the hierarchy, the drawing
    /// tools for that view, and zoom.
    pub(super) fn canvas_header(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let mut prompt = None;
        let mut error = None;
        ui.horizontal(|ui| {
            ui.set_height(38.0);
            ui.add_space(8.0);
            let has_schematic = ws.has_schematic();
            let active = match (self.show_netlist, ws.mode) {
                (true, _) => 2,
                (false, Mode::Schematic) => 0,
                (false, Mode::Symbol) => 1,
            };
            let views =
                [("Schematic", has_schematic), ("Symbol", true), ("Netlist", has_schematic)];
            match style::segmented(ui, &views, active) {
                Some(0) => {
                    self.show_netlist = false;
                    ws.set_mode(Mode::Schematic);
                }
                Some(1) => {
                    self.show_netlist = false;
                    ws.set_mode(Mode::Symbol);
                }
                Some(2) => {
                    ws.set_mode(Mode::Schematic);
                    self.show_netlist = true;
                }
                _ => {}
            }
            // Where in the hierarchy, when inside a part.
            if ws.levels.len() > 1 {
                ui.add_space(8.0);
                let mut go_to = None;
                for (depth, level) in ws.levels.iter().enumerate() {
                    if depth > 0 {
                        ui.label(RichText::new(icon::CARET_RIGHT).color(p.weak));
                    }
                    let label = match &level.via {
                        Some(inst) => format!("{inst} ({})", level.cell.cell),
                        None => level.cell.cell.clone(),
                    };
                    let current = depth + 1 == ws.levels.len();
                    if ui.selectable_label(current, label).clicked() && !current {
                        go_to = Some(depth + 1);
                    }
                }
                if let Some(depth) = go_to {
                    ws.ascend_to(depth);
                }
            }
            if !ws.editable() {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{}  read-only", icon::PROHIBIT)).color(p.weak));
                if let Some(c) = ws.back_to().cloned() {
                    if ui
                        .button(format!("{}  Back to {}", icon::ARROW_LEFT, c.cell))
                        .on_hover_text("Alt+Left")
                        .clicked()
                    {
                        ws.go_back();
                    }
                }
            }

            style::trailing(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(8.0);
                let view = self.canvas.view;
                let level = ws.levels.last_mut().expect("level");
                let pct = level.camera.map_or(100.0, |c| c.scale / ZOOM_100 * 100.0);
                if icon_button(ui, icon::CORNERS_OUT, "Fit to window  (F)", true, false).clicked() {
                    level.camera = None;
                }
                let mut zoom = |factor: f32| {
                    if let (Some(cam), Some(view)) = (&mut level.camera, view) {
                        cam.zoom_about(view, view.center(), factor);
                        cam.fitted = None;
                    }
                };
                if icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom in  (wheel)", true, false)
                    .clicked()
                {
                    zoom(1.25);
                }
                ui.label(RichText::new(format!("{pct:.0}%")).monospace().color(p.text));
                if icon_button(ui, icon::MAGNIFYING_GLASS_MINUS, "Zoom out  (wheel)", true, false)
                    .clicked()
                {
                    zoom(0.8);
                }
                if self.show_netlist {
                    return;
                }
                toolbar_gap(ui);
                // Tools, right to left.
                let editable = ws.editable();
                let current = ws.tool.name();
                let tool = |ui: &mut egui::Ui, glyph: &str, name: &str, tip: &str| {
                    icon_button(ui, glyph, tip, editable, current == name).clicked()
                };
                if ws.mode == Mode::Symbol {
                    let has_ports = ws.current_def().is_some_and(|d| !d.cell.ports.is_empty());
                    if icon_button(
                        ui,
                        icon::SPARKLE,
                        "Generate: replace the drawing with a box made from the ports",
                        editable && has_ports,
                        false,
                    )
                    .clicked()
                    {
                        let _ = ws.edit(|_, _, d| {
                            symbol_edit::generate(d);
                            Ok(())
                        });
                    }
                    if tool(
                        ui,
                        icon::DOT_OUTLINE,
                        "Pin",
                        "Pin  (P): place pins for ports without one",
                    ) {
                        if let Err(e) = ws.start_pin(None) {
                            error = Some(e);
                        }
                    }
                    if tool(ui, icon::TEXT_T, "Text", "Text  (T)") {
                        prompt = Some(Prompt::SymbolText { text: String::new() });
                    }
                    if tool(ui, icon::CIRCLE_HALF, "Arc", "Arc  (A)") {
                        ws.tool = Tool::Arc { center: None, start: None };
                    }
                    if tool(ui, icon::CIRCLE, "Circle", "Circle  (C)") {
                        ws.tool = Tool::Circle { center: None };
                    }
                    if tool(ui, icon::RECTANGLE, "Rectangle", "Rectangle  (B)") {
                        ws.tool = Tool::Rect { from: None };
                    }
                    if tool(ui, icon::LINE_SEGMENT, "Line", "Line  (L)") {
                        ws.tool = Tool::Line { points: Vec::new() };
                    }
                } else {
                    if tool(ui, icon::PLUS, "Place", "Add part  (A)") {
                        prompt = Some(Prompt::AddPart { query: String::new() });
                    }
                    if tool(
                        ui,
                        icon::PLUG,
                        "Port",
                        "Port  (P): a connection to the sheets that use this cell",
                    ) {
                        prompt = Some(new_port_prompt(true));
                    }
                    if tool(ui, icon::TAG, "Label", "Net label  (L)") {
                        prompt = Some(Prompt::Label { net: String::new() });
                    }
                    if tool(ui, icon::LINE_SEGMENTS, "Wire", "Wire  (W), or drag from a pin's end")
                    {
                        ws.tool = Tool::Wire { points: Vec::new(), horizontal_first: true };
                    }
                }
                if tool(ui, icon::CURSOR, "Select", "Select  (Esc)") {
                    ws.tool = Tool::Select;
                }
            });
        });
        if prompt.is_some() {
            self.prompt = prompt;
        }
        if error.is_some() {
            self.error = error;
        }
    }

    /// The current cell's netlist, as the simulator sees it.
    pub(super) fn netlist_view(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &self.workspace else { return };
        let id = ws.current().cell.clone();
        let stale = !matches!(&self.netlist, Some((c, r, _)) if *c == id && *r == ws.revision);
        if stale {
            let text = match circuiter_netlist::Netlist::build(&ws.design, &id) {
                Ok(n) => n.to_spice(),
                Err(e) => format!("* {e}"),
            };
            self.netlist = Some((id, ws.revision, text));
        }
        let Some((_, _, text)) = &self.netlist else { return };
        let p = style::palette(ui);
        egui::Frame::new().fill(p.well).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
                for (n, line) in text.lines().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:>4}", n + 1))
                                .monospace()
                                .color(p.weak.gamma_multiply(0.6)),
                        );
                        let color = if line.starts_with('*') {
                            p.weak
                        } else if line.starts_with('.') {
                            p.accent
                        } else {
                            p.text
                        };
                        ui.label(RichText::new(line).monospace().color(color));
                    });
                }
            });
        });
    }
}
