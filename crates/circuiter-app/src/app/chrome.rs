//! The frame around the sheet: title bar, toolbar, document tabs, the bar above the
//! canvas with its view switcher and drawing tools, and the status bar.

use super::*;
use crate::style::{self, icon, icon_button, toolbar_gap};

/// What the live controls asked for.
#[derive(Debug, Clone, Copy)]
pub(super) enum LiveAction {
    Start,
    Stop,
    Pause(bool),
    Speed(f64),
}

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
            // The menus keep to their own width, leaving room for the workspaces.
            ui.allocate_ui_with_layout(
                egui::vec2(430.0, 40.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| self.menu_bar(ui),
            );
            ui.add_space(12.0);
            self.space_switcher(ui);
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

    /// The workspaces, one tab each: Schematic, PCB, 3D and Simulation.
    fn space_switcher(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &self.workspace else { return };
        let p = style::palette(ui);
        let sheet = ws.has_schematic();
        let now = self.current_space();
        let mut go = None;
        for space in Space::ALL {
            let glyph = match space {
                Space::Schematic => icon::CIRCUITRY,
                Space::Pcb => icon::CPU,
                Space::Model => icon::CUBE,
                Space::Simulation => icon::WAVE_SINE,
            };
            let on = space == now;
            let enabled = sheet || matches!(space, Space::Schematic | Space::Model);
            let text = RichText::new(format!("{glyph}  {}", space.name()))
                .size(14.0)
                .color(if on { p.text } else { p.weak });
            let tab = egui::Button::new(if on { text.strong() } else { text })
                .fill(if on { p.accent.gamma_multiply(0.25) } else { egui::Color32::TRANSPARENT })
                .stroke(egui::Stroke::new(
                    1.0,
                    if on { p.accent.gamma_multiply(0.7) } else { egui::Color32::TRANSPARENT },
                ))
                .corner_radius(6)
                .min_size(egui::vec2(0.0, 28.0));
            let tip = match space {
                Space::Schematic => "Schematic workspace: draw the circuit, its parts and symbols",
                Space::Pcb => "PCB workspace: lay out and route the board",
                Space::Model => "3D workspace: model parts and see the product in 3D",
                Space::Simulation => {
                    "Simulation workspace: run the circuit, the bench and the results"
                }
            };
            if ui.add_enabled(enabled, tab).on_hover_text(tip).clicked() && !on {
                go = Some(space);
            }
        }
        if let Some(space) = go {
            self.set_space(space);
        }
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
        let mut live_action = None;
        ui.horizontal(|ui| {
            ui.set_height(40.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(8.0);
            let has_ws = self.workspace.is_some();
            if icon_button(ui, icon::FILE_PLUS, "New cell  (Ctrl+N)", has_ws, false).clicked() {
                prompt = self.workspace.as_ref().map(|ws| new_cell_prompt(ws, false));
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
            let parts = sheet && ws.selection.iter().any(|o| matches!(o, Owner::Instance(_)));
            if icon_button(
                ui,
                icon::PUZZLE_PIECE,
                "Make a component from the selected parts",
                parts,
                false,
            )
            .clicked()
            {
                prompt = Some(make_component_prompt(ws));
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
                let plan = ws.run_plan();
                let tstop = plan.tstop;
                let (text, tip) = if plan.until_halt {
                    (
                        "Time: until ECALL".to_owned(),
                        format!(
                            "Run goes until the program ends (ECALL), or {} at most. \
                             Click to run for a set time instead",
                            show(tstop, "s")
                        ),
                    )
                } else {
                    (
                        format!("Time: {}", show(tstop, "s")),
                        "How long Run watches the circuit. Click to change it".to_owned(),
                    )
                };
                let time = egui::Button::new(
                    RichText::new(format!("{text}  {}", icon::CARET_DOWN)).color(p.text),
                )
                .min_size(egui::vec2(120.0, 28.0));
                if ui.add(time).on_hover_text(tip).clicked() {
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
                ui.add_space(8.0);
                // Live: the circuit keeps running until stopped, and can be operated.
                match &ws.live {
                    Some(_) => {
                        ui.label(
                            RichText::new(format!("{}  LIVE", icon::BROADCAST))
                                .color(p.error)
                                .strong(),
                        )
                        .on_hover_text(
                            "The circuit is running live: its controls are above the sheet",
                        );
                    }
                    None => {
                        let live = egui::Button::new(
                            RichText::new(format!("{}  Run Live", icon::BROADCAST))
                                .color(p.accent)
                                .strong(),
                        )
                        .stroke(egui::Stroke::new(1.0, p.accent.gamma_multiply(0.6)))
                        .min_size(egui::vec2(120.0, 28.0));
                        if ui
                            .add(live)
                            .on_hover_text(
                                "Keep the circuit running until you stop it: flip switches, hold \
                                 buttons and turn knobs while it runs, and watch the meters and \
                                 the oscilloscope",
                            )
                            .clicked()
                        {
                            live_action = Some(LiveAction::Start);
                        }
                    }
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
        if let Some(action) = live_action {
            self.live_action(action);
        }
        if transient {
            self.transient_prompt();
        }
    }

    /// The open cells, then the open programs, one tab each.
    pub(super) fn doc_tabs(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let tabs = ws.tabs();
        let active = ws.active_tab();
        let showing_code = self.active_code.is_some();
        let mut switch = None;
        let mut close = None;
        let mut code_switch = None;
        let mut code_close = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for (i, id) in tabs.iter().enumerate() {
                let schematic = ws.design.cell(id).is_some_and(|d| d.schematic.is_some());
                let glyph = if schematic { icon::CIRCUITRY } else { icon::CPU };
                let name =
                    if id.library == ws.project.name() { id.cell.clone() } else { id.to_string() };
                let on = i == active && !showing_code;
                let (clicked, closed) =
                    doc_tab(ui, glyph, &name, &id.to_string(), on, tabs.len() > 1, ("cell", i));
                if clicked {
                    switch = Some(i);
                }
                if closed {
                    close = Some(i);
                }
            }
            for (i, tab) in self.code_tabs.iter().enumerate() {
                let name = if tab.dirty() { format!("{} ●", tab.name()) } else { tab.name() };
                let on = self.active_code == Some(i);
                let tip = tab.path.display().to_string();
                let (clicked, closed) =
                    doc_tab(ui, icon::FILE_TEXT, &name, &tip, on, true, ("code", i));
                if clicked {
                    code_switch = Some(i);
                }
                if closed {
                    code_close = Some(i);
                }
            }
        });
        if let Some(i) = close {
            ws.close_tab(i);
        } else if let Some(i) = switch {
            ws.switch_tab(i);
            self.active_code = None;
        }
        if let Some(i) = code_close {
            self.close_code(i);
        } else if let Some(i) = code_switch {
            self.active_code = Some(i);
        }
    }

    /// Above the canvas: which view of the cell, where in the hierarchy, the drawing
    /// tools for that view, and zoom.
    pub(super) fn canvas_header(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let p = style::palette(ui);
        let mut prompt = None;
        let mut error = None;
        // Narrow: the tools go on a row of their own under the view switcher.
        let narrow = ui.available_width() < 720.0;
        let layout = if narrow {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        ui.with_layout(layout, |ui| {
            ui.horizontal(|ui| {
                ui.set_height(38.0);
                ui.add_space(8.0);
                let has_schematic = ws.has_schematic();
                // The schematic workspace's views; the others have their own bars.
                if !(self.show_pcb || self.show_3d || self.simulating) {
                    {
                        let active = match (self.show_netlist, self.show_diagram, ws.mode) {
                            (true, _, _) => 2,
                            (_, true, _) => 3,
                            (_, _, Mode::Schematic) => 0,
                            (_, _, Mode::Symbol) => 1,
                        };
                        let views = [
                            ("Schematic", has_schematic),
                            ("Symbol", true),
                            ("Netlist", has_schematic),
                            ("Diagram", has_schematic),
                        ];
                        let picked = style::segmented(ui, &views, active);
                        if picked.is_some() {
                            (self.show_netlist, self.show_diagram) = (false, false);
                        }
                        match picked {
                            Some(0) => ws.set_mode(Mode::Schematic),
                            Some(1) => ws.set_mode(Mode::Symbol),
                            Some(2) => {
                                ws.set_mode(Mode::Schematic);
                                self.show_netlist = true;
                            }
                            Some(3) => {
                                ws.set_mode(Mode::Schematic);
                                self.show_diagram = true;
                            }
                            _ => {}
                        }
                    }
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
            });
            style::trailing(ui, |ui| {
                // The 3D and board views have their own controls.
                if self.show_3d || self.show_pcb {
                    return;
                }
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(8.0);
                // The diagram has its own view of things.
                let (camera, view) = if self.show_diagram {
                    (&mut self.diagram.camera, self.diagram.canvas.view)
                } else {
                    (&mut ws.levels.last_mut().expect("level").camera, self.canvas.view)
                };
                let pct = camera.map_or(100.0, |c| c.scale / ZOOM_100 * 100.0);
                if icon_button(ui, icon::CORNERS_OUT, "Fit to window  (F)", true, false).clicked() {
                    *camera = None;
                }
                let mut zoom = |factor: f32| {
                    if let (Some(cam), Some(view)) = (camera.as_mut(), view) {
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
                if self.show_diagram && ws.editable() {
                    toolbar_gap(ui);
                    let laid = self.diagram.laid_out(&ws.current().cell, ws.revision);
                    let r = ui
                        .add_enabled(laid.is_some(), egui::Button::new("Use This Layout"))
                        .on_hover_text(
                            "Make this layout the sheet's own: parts and wires move to where \
                             they are here (undo with Ctrl+Z). Group boxes are dropped and \
                             notes stacked under the drawing.",
                        );
                    if let Some(laid) = laid.filter(|_| r.clicked()) {
                        match ws.use_layout(laid) {
                            Ok(()) => self.show_diagram = false,
                            Err(e) => error = Some(e),
                        }
                    }
                }
                if self.show_netlist || self.show_diagram || self.show_3d {
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
                    if tool(ui, icon::NOTE, "Note", "Note  (T): text on the sheet") {
                        prompt = Some(Prompt::Note { text: String::new() });
                    }
                    if tool(
                        ui,
                        icon::SELECTION,
                        "Group box",
                        "Group box  (B): a dashed box around a group of parts",
                    ) {
                        ws.tool = Tool::GroupBox { from: None };
                    }
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
                    if tool(
                        ui,
                        icon::X,
                        "No connect",
                        "No-connect flag  (Q): marks a pin as meant to be left open",
                    ) {
                        ws.tool = Tool::NoConnect;
                    }
                    if tool(ui, icon::TAG, "Label", "Net label  (L)") {
                        prompt = Some(Prompt::Label { net: String::new(), global: false });
                    }
                    if tool(ui, icon::LINE_SEGMENTS, "Wire", "Wire  (W), or drag from a pin's end")
                    {
                        ws.tool = Tool::Wire { points: Vec::new(), horizontal_first: true };
                    }
                }
                if icon_button(
                    ui,
                    icon::HAND,
                    "Hand  (H): drag to move the view. Middle or right drag, or Space+drag, \
                     also do",
                    true,
                    current == "Pan",
                )
                .clicked()
                {
                    ws.tool = if ws.tool == Tool::Pan { Tool::Select } else { Tool::Pan };
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

/// One tab: an icon and a name, marked when in front, with a close button. Returns
/// (clicked, close clicked); a middle click closes too.
fn doc_tab(
    ui: &mut egui::Ui,
    glyph: &str,
    name: &str,
    tip: &str,
    on: bool,
    closable: bool,
    id: (&str, usize),
) -> (bool, bool) {
    let p = style::palette(ui);
    let text = format!("{glyph}  {name}");
    let galley =
        egui::WidgetText::from(RichText::new(&text).color(if on { p.text } else { p.weak }))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            );
    let width = galley.size().x + if closable { 52.0 } else { 28.0 };
    let (rect, r) = ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::click());
    let fill = if on {
        p.panel
    } else if r.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        p.raised
    };
    ui.painter().rect_filled(rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
    if on {
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0));
        ui.painter().rect_filled(bar, 0.0, p.accent);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
        galley,
        p.text,
    );
    let r = r.on_hover_text(tip);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    let mut closed = r.middle_clicked();
    if closable {
        let x = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        );
        let xr = ui.interact(x, ui.id().with(("close-tab", id)), egui::Sense::click());
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
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Close {name}"))
        });
        closed |= xr.clicked();
    }
    (r.clicked(), closed)
}

impl CircuiterApp {
    pub(super) fn live_action(&mut self, action: LiveAction) {
        // Programs are read from disk when a run starts.
        if matches!(action, LiveAction::Start) {
            self.save_code();
        }
        let Some(ws) = &mut self.workspace else { return };
        match action {
            LiveAction::Start => {
                let speed = self
                    .live_speed
                    .unwrap_or_else(|| crate::live::default_speed(ws.run_plan().tstop));
                match ws.start_live(speed) {
                    Ok(()) => self.show_bottom = true,
                    Err(e) => self.error = Some(format!("can't run live: {e}")),
                }
            }
            LiveAction::Stop => ws.stop_live(),
            LiveAction::Pause(p) => {
                if let Some(live) = &ws.live {
                    live.send(crate::live::Command::Pause(p));
                    if let Ok(mut s) = live.shared.lock() {
                        s.paused = p;
                    }
                }
            }
            LiveAction::Speed(s) => {
                self.live_speed = Some(s);
                if let Some(live) = &ws.live {
                    live.send(crate::live::Command::Speed(s));
                }
            }
        }
    }
}

impl CircuiterApp {
    /// While running live: a strip above the sheet with the run's controls.
    pub(super) fn live_bar(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &self.workspace else { return };
        let Some(live) = &ws.live else { return };
        let p = style::palette(ui);
        let (time, speed, load, paused) = live
            .shared
            .lock()
            .map(|s| (s.time, s.speed, s.load, s.paused))
            .unwrap_or((0.0, 1.0, 0.0, false));
        let stale = ws.revision != ws.live_revision;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.add_space(10.0);
            // A dot that pulses while running.
            let pulse = if paused {
                0.4
            } else {
                0.55 + 0.45 * ((ui.input(|i| i.time) * 3.0).sin() as f32).abs()
            };
            ui.label(RichText::new("●").color(p.error.gamma_multiply(pulse)).size(16.0));
            ui.label(RichText::new(if paused { "PAUSED" } else { "LIVE" }).strong().color(p.text));
            ui.add_space(6.0);
            ui.label(RichText::new(format!("t = {}", show(time, "s"))).monospace().color(p.text))
                .on_hover_text("Simulated time since the live run started");
            if load > 1.05 && !paused {
                ui.label(
                    RichText::new(format!("{}  can't keep up: slower than set", icon::WARNING))
                        .color(p.warning),
                )
                .on_hover_text(
                    "The circuit needs more computing than this speed allows; choose a lower speed",
                );
            }
            ui.add_space(8.0);
            let (glyph, tip) = if paused { (icon::PLAY, "Resume") } else { (icon::PAUSE, "Pause") };
            if icon_button(ui, glyph, tip, true, paused).clicked() {
                action = Some(LiveAction::Pause(!paused));
            }
            ui.label(RichText::new("Speed").color(p.weak));
            egui::ComboBox::from_id_salt("live-speed")
                .selected_text(format!("{}/s", show(speed, "s")))
                .width(90.0)
                .show_ui(ui, |ui| {
                    for s in crate::live::SPEEDS {
                        let label = format!("{}/s", show(s, "s"));
                        if ui.selectable_label((s - speed).abs() < s * 1e-6, label).clicked() {
                            action = Some(LiveAction::Speed(s));
                        }
                    }
                })
                .response
                .on_hover_text("Simulated time per real second: 1 s/s is real time");
            if stale {
                let restart = egui::Button::new(
                    RichText::new(format!("{}  Restart with changes", icon::ARROWS_CLOCKWISE))
                        .color(p.warning),
                );
                if ui
                    .add(restart)
                    .on_hover_text("The design changed since the run started")
                    .clicked()
                {
                    action = Some(LiveAction::Start);
                }
            }
            style::trailing(ui, |ui| {
                ui.add_space(10.0);
                let stop = egui::Button::new(
                    RichText::new(format!("{}  Stop Live", icon::STOP)).color(p.error).strong(),
                );
                if ui.add(stop).clicked() {
                    action = Some(LiveAction::Stop);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new("Click switches · hold buttons · knobs in Properties")
                            .color(p.weak),
                    )
                    .truncate(),
                );
            });
        });
        if let Some(a) = action {
            self.live_action(a);
        }
    }
}
