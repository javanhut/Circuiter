//! Window layout: menus, toolbars, library browser, inspector, prompts and status bar.

mod bench;
mod body_editor;
mod chrome;
mod code;
mod diagram;
mod explorer;
mod footprint;
mod panels;
pub(crate) mod pcb;
mod viewer3d;

use crate::Args;
use crate::canvas::{self, CanvasState, Highlight};
use crate::plot::{self, PlotState, Trace, XAxis};
use crate::simulation::{Results, show};
use crate::style;
use crate::workspace::{Analysis, Mode, Tool, Workspace, graphic_name};
use crate::{interface, symbol_edit};
use circuiter_model::builder;
use circuiter_model::cell::{ParamDef, ParamKind, PortKind};
use circuiter_model::check::Severity;
use circuiter_model::connectivity::{Item, NetNameSource};
use circuiter_model::symbol::{Anchor, Graphic, PinDirection};
use circuiter_model::{CellId, Length, ViewKind};
use circuiter_scene::{Owner, Pt, Theme, param_display, to_svg};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, ViewportCommand};
use explorer::DraggedCell;
#[cfg(test)]
pub use explorer::folder_label;
use std::path::{Path, PathBuf};

const ERROR: Color32 = Color32::from_rgb(230, 80, 70);
const WARNING: Color32 = Color32::from_rgb(230, 170, 60);
const OK: Color32 = Color32::from_rgb(90, 190, 120);
const ACCENT: Color32 = Color32::from_rgb(79, 179, 255);

pub struct CircuiterApp {
    workspace: Option<Workspace>,
    error: Option<String>,
    theme: &'static Theme,
    applied_theme: Option<&'static str>,
    show_grid: bool,
    cursor: Option<Pt>,
    hovered: Option<Owner>,
    canvas: CanvasState,
    prompt: Option<Prompt>,
    field: Field,
    title: String,
    allow_close: bool,
    screenshot: Option<PathBuf>,
    frames: u32,
    plot: PlotState,
    /// Plot part temperatures instead of signals after a transient run.
    plot_heat: bool,
    /// Symbols drawn as icons (parts lists, the palette), by cell.
    icons: std::collections::BTreeMap<CellId, circuiter_scene::Scene>,
    /// The design revision the project cells' icons were drawn from.
    icons_revision: u64,
    /// Which tab of the left, right and bottom panels is showing.
    left_tab: usize,
    right_tab: usize,
    bottom_tab: usize,
    show_bottom: bool,
    /// The Netlist view instead of the sheet.
    show_netlist: bool,
    /// The sheet laid out automatically instead of as drawn.
    show_diagram: bool,
    diagram: diagram::DiagramView,
    /// Watches the open project's files for changes made by other programs.
    watch: Option<crate::watch::Watch>,
    /// Merges of changes on disk that failed in a row: a file caught half written fails
    /// once, a broken one keeps failing.
    disk_failures: u32,
    /// Showing the 3D view instead of the sheet.
    show_3d: bool,
    /// Showing the board editor instead of the sheet.
    show_pcb: bool,
    pcb: pcb::PcbView,
    viewer3d: viewer3d::Viewer3d,
    /// The netlist shown, for which cell and design revision.
    netlist: Option<(CellId, u64, String)>,
    /// Filters the components list.
    component_query: String,
    /// Programs open in tabs, and the one in front (over the sheet), if any.
    code_tabs: Vec<code::CodeTab>,
    active_code: Option<usize>,
    /// While running live: the press began on a part being operated (so it isn't a
    /// drag), and the momentary part held down.
    live_press: bool,
    live_held: Option<String>,
    /// Each oscilloscope's front panel, by instance path.
    scopes: std::collections::BTreeMap<String, bench::ScopeSettings>,
    /// Simulated seconds per real second for live runs, once chosen.
    live_speed: Option<f64>,
    /// A part dragged in from the library is being previewed on the sheet.
    drop_preview: bool,
    /// The footprint editor, when a footprint is open in it.
    footprint_editor: footprint::Editor,
    /// The Simulation workspace was chosen (the sheet, with the bench around it).
    simulating: bool,
    /// The workspace whose panels are showing, and each workspace's panels while
    /// another is in front.
    space: Space,
    spaces: [Panels; 4],
    /// A simulation or live run was going last frame.
    was_running: bool,
    /// The net whose menu is open, from a right-click on one of its wires.
    net_menu: Option<usize>,
}

/// The workspaces: places of their own for each part of the job, swapped between at
/// the top of the window, each with its own panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    /// Drawing the circuit: the sheet, its parts and symbols.
    Schematic,
    /// Laying out the board.
    Pcb,
    /// Modelling parts and seeing the product in 3D.
    Model,
    /// Running the circuit: analyses, the bench, results.
    Simulation,
}

impl Space {
    pub const ALL: [Space; 4] = [Space::Schematic, Space::Pcb, Space::Model, Space::Simulation];

    pub fn name(self) -> &'static str {
        match self {
            Space::Schematic => "Schematic",
            Space::Pcb => "PCB",
            Space::Model => "3D",
            Space::Simulation => "Simulation",
        }
    }

    fn index(self) -> usize {
        Space::ALL.iter().position(|s| *s == self).expect("listed")
    }
}

/// What a workspace's panels show.
#[derive(Debug, Clone, Copy)]
struct Panels {
    left_tab: usize,
    right_tab: usize,
    bottom_tab: usize,
    show_bottom: bool,
}

impl Panels {
    /// How each workspace starts: the simulation's on its own panel with waveforms
    /// below, the schematic with its problems, the 3D view with nothing below.
    fn first(space: Space) -> Panels {
        let p = Panels { left_tab: 0, right_tab: 0, bottom_tab: 0, show_bottom: true };
        match space {
            Space::Schematic => Panels { bottom_tab: 2, show_bottom: false, ..p },
            Space::Pcb => p,
            Space::Model => Panels { show_bottom: false, ..p },
            Space::Simulation => Panels { right_tab: 1, ..p },
        }
    }
}

/// The parts on the quick-parts strip, with their shortcut keys if any.
const PALETTE: &[(&str, &str)] = &[
    ("resistor", ""),
    ("capacitor", ""),
    ("inductor", ""),
    ("diode", ""),
    ("led", ""),
    ("npn", ""),
    ("pnp", ""),
    ("nmos", ""),
    ("pmos", ""),
    ("vdc", ""),
    ("vsine", ""),
    ("vpulse", ""),
    ("clock", ""),
    ("gnd", "G"),
    ("vdd", "Shift+V"),
    ("not", ""),
    ("and", ""),
    ("or", ""),
    ("nand", ""),
    ("nor", ""),
    ("xor", ""),
    ("xnor", ""),
];

/// Every keyboard shortcut, for Help.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+R", "Run the simulation"),
    ("H", "Hand: drag to move the view (or middle/right drag, Space+drag)"),
    ("F9 / F10", "Operating point / transient"),
    ("Ctrl+K, A", "Search for a part to place"),
    ("G, Shift+V", "Place ground / VDD"),
    ("W", "Wire (or drag from a pin's end)"),
    ("L", "Net label"),
    ("Q", "No-connect flag"),
    ("P", "Port"),
    ("R, X", "Rotate, mirror"),
    ("Del", "Delete"),
    ("Ctrl+C, X, V, D", "Copy, cut, paste, duplicate"),
    ("Ctrl+Z, Ctrl+Y", "Undo, redo"),
    ("V", "Schematic / symbol"),
    ("F", "Fit to window"),
    ("Alt+Left, Alt+Right", "Back, forward"),
    ("Alt+Up", "Up a level"),
    ("Ctrl+N, Ctrl+S, Ctrl+O", "New cell, save, open project"),
    ("Esc", "Cancel, or stop a simulation"),
    ("PCB: X", "Route; Ctrl+click finishes the connection automatically"),
    (
        "PCB: Shift+R, Shift+Space, Space",
        "Routing mode (push and shove, walkaround, stop, ignore); corner style (45°, 90°, any); flip the corner",
    ),
    ("PCB: 3, V", "Cycle track width; drop a via"),
    ("PCB: PgUp/PgDn, Ctrl+Shift+wheel", "Change copper layer (with a via while routing)"),
    ("PCB: Shift+S, F5", "Single-layer view; net colours"),
    ("PCB: R, F, L", "Turn, flip to the other side, lock the selected part"),
    ("PCB: Ctrl+M", "Measure"),
];

/// A modal dialog.
enum Prompt {
    AddPart {
        query: String,
    },
    Label {
        net: String,
        global: bool,
    },
    /// Renumbering the sheet's parts: across rows or down columns, keeping numbers or
    /// starting over.
    Annotate {
        columns: bool,
        reset: bool,
    },
    /// A new port: placed as a flag on the schematic (`place`), or added to the
    /// interface directly.
    Port {
        name: String,
        kind: PortKind,
        place: bool,
        error: Option<String>,
    },
    SymbolText {
        text: String,
    },
    /// A note's text, before placing it on the sheet.
    Note {
        text: String,
    },
    Transient {
        stop: String,
        step: String,
        power_on: bool,
        error: Option<String>,
    },
    Ac {
        start: String,
        stop: String,
        per_decade: String,
        error: Option<String>,
    },
    /// Choosing which models in a library file to import, then the outcome.
    Import {
        file: String,
        defs: Vec<circuiter_sim::parse::Definition>,
        selected: Vec<bool>,
        outcome: Option<Vec<crate::workspace::ImportOutcome>>,
    },
    AddParam {
        name: String,
        kind: ParamChoice,
        unit: String,
        options: String,
        default: String,
        error: Option<String>,
    },
    NewCell {
        name: String,
        folder: String,
        error: Option<String>,
        /// A 3D part (a body to model) rather than a circuit.
        part3d: bool,
    },
    /// A new folder in `parent`, or renaming folder `renaming`.
    Folder {
        lib: String,
        parent: String,
        renaming: Option<String>,
        name: String,
        error: Option<String>,
    },
    MoveCell {
        id: CellId,
        folder: String,
        error: Option<String>,
    },
    Shortcuts,
    NewProgram {
        name: String,
        error: Option<String>,
    },
    /// A new component from the selected parts.
    MakeComponent {
        name: String,
        folder: String,
        error: Option<String>,
    },
    /// Unsaved changes would be lost by `then`.
    Discard {
        then: Pending,
    },
}

/// The kinds of parameter offered when adding one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParamChoice {
    Real,
    Integer,
    Bool,
    Text,
    Choice,
}

impl ParamChoice {
    const ALL: [ParamChoice; 5] = [
        ParamChoice::Real,
        ParamChoice::Integer,
        ParamChoice::Bool,
        ParamChoice::Text,
        ParamChoice::Choice,
    ];

    fn label(self) -> &'static str {
        match self {
            ParamChoice::Real => "Quantity",
            ParamChoice::Integer => "Integer",
            ParamChoice::Bool => "Yes/no",
            ParamChoice::Text => "Text",
            ParamChoice::Choice => "Choice",
        }
    }
}

const PORT_KINDS: [PortKind; 6] = [
    PortKind::Input,
    PortKind::Output,
    PortKind::Bidirectional,
    PortKind::Passive,
    PortKind::PowerIn,
    PortKind::PowerOut,
];

fn kind_label(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Input => "Input",
        PortKind::Output => "Output",
        PortKind::Bidirectional => "Bidirectional",
        PortKind::Passive => "Passive",
        PortKind::PowerIn => "Power in",
        PortKind::PowerOut => "Power out",
    }
}

fn param_kind_label(kind: &ParamKind) -> String {
    match kind {
        ParamKind::Real { unit } if unit.is_empty() => "quantity".into(),
        ParamKind::Real { unit } => format!("quantity, {unit}"),
        ParamKind::Integer => "integer".into(),
        ParamKind::Bool => "yes/no".into(),
        ParamKind::Text => "text".into(),
        ParamKind::Choice(options) => format!("one of {}", options.join(", ")),
    }
}

/// The side of the body a pin is on, which is opposite the way it points.
fn side_label(direction: PinDirection) -> &'static str {
    match direction {
        PinDirection::Right => "Left",
        PinDirection::Left => "Right",
        PinDirection::Down => "Top",
        PinDirection::Up => "Bottom",
    }
}

/// Parses a length typed by the user: millimetres, or mils with a `mil` suffix.
fn parse_length(text: &str) -> Result<Length, String> {
    let t = text.trim();
    let (number, mils) = match t.strip_suffix("mil") {
        Some(n) => (n, true),
        None => (t.strip_suffix("mm").unwrap_or(t), false),
    };
    let v: f64 = number.trim().parse().map_err(|_| format!("`{t}` is not a length"))?;
    if !(v > 0.0 && v.is_finite()) {
        return Err("the length must be positive".into());
    }
    Ok(if mils { Length::from_mm(v * 0.0254) } else { Length::from_mm(v) })
}

fn parse_degrees(text: &str) -> Result<f64, String> {
    let t = text.trim().trim_end_matches('°');
    t.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(symbol_edit::normalize)
        .ok_or_else(|| format!("`{t}` is not an angle in degrees"))
}

#[derive(Clone)]
enum Pending {
    Quit,
    Reload,
    OpenDialog,
    NewProjectDialog,
}

/// The inspector text field being edited, with its working text and any error.
#[derive(Default)]
struct Field {
    key: Option<String>,
    buffer: String,
    error: Option<(String, String)>,
}

impl Field {
    /// A single-line field showing `current`. Returns the new text when the user commits a
    /// change with Enter or by clicking away. Escape cancels.
    fn edit(&mut self, ui: &mut egui::Ui, key: &str, current: &str, width: f32) -> Option<String> {
        self.edit_hinted(ui, key, current, width, "")
    }

    /// [`Field::edit`] with greyed-out `hint` text in the field while it is empty.
    fn edit_hinted(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        current: &str,
        width: f32,
        hint: &str,
    ) -> Option<String> {
        let editing = self.key.as_deref() == Some(key);
        let mut shown = current.to_owned();
        let text = if editing { &mut self.buffer } else { &mut shown };
        // Sized explicitly: inside a grid, a text field otherwise shrinks to the width
        // its column had on the previous frame.
        let size = egui::vec2(width, ui.spacing().interact_size.y);
        let r = ui.add_sized(
            size,
            egui::TextEdit::singleline(text)
                .desired_width(width)
                .font(egui::TextStyle::Monospace)
                .hint_text(hint),
        );
        if r.gained_focus() {
            self.key = Some(key.to_owned());
            self.buffer = current.to_owned();
        }
        if r.lost_focus() && editing {
            self.key = None;
            let text = std::mem::take(&mut self.buffer);
            let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
            if !cancelled && text != current {
                return Some(text);
            }
        }
        if let Some((k, msg)) = &self.error {
            if k == key {
                ui.colored_label(ERROR, msg);
            }
        }
        None
    }
}

impl CircuiterApp {
    #[cfg(test)]
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    #[cfg(test)]
    pub fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspace.as_mut()
    }

    #[cfg(test)]
    pub fn code_tab_mut(&mut self, path: &str) -> Option<&mut code::CodeTab> {
        self.code_tabs.iter_mut().find(|t| t.path == Path::new(path))
    }

    #[cfg(test)]
    pub fn pcb_view(&self) -> &pcb::PcbView {
        &self.pcb
    }

    #[cfg(test)]
    pub fn pcb_view_mut(&mut self) -> &mut pcb::PcbView {
        &mut self.pcb
    }

    #[cfg(test)]
    pub fn canvas_view(&self) -> Option<egui::Rect> {
        self.canvas.view
    }

    pub fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        style::install_fonts(&cc.egui_ctx);
        let theme = args
            .theme
            .as_deref()
            .and_then(|t| Theme::ALL.into_iter().find(|th| th.name.eq_ignore_ascii_case(t)))
            .unwrap_or(&Theme::BENCH);
        let mut app = CircuiterApp {
            workspace: None,
            error: None,
            theme,
            applied_theme: None,
            show_grid: true,
            cursor: None,
            hovered: None,
            canvas: CanvasState::default(),
            prompt: None,
            field: Field::default(),
            title: String::new(),
            allow_close: false,
            screenshot: args.screenshot,
            frames: 0,
            plot: PlotState::default(),
            plot_heat: args.heat,
            icons: std::collections::BTreeMap::new(),
            icons_revision: 0,
            left_tab: 0,
            right_tab: 0,
            bottom_tab: 0,
            show_bottom: true,
            show_netlist: false,
            show_diagram: false,
            diagram: diagram::DiagramView::default(),
            watch: None,
            disk_failures: 0,
            show_3d: false,
            show_pcb: false,
            simulating: false,
            footprint_editor: footprint::Editor::default(),
            space: Space::Schematic,
            spaces: Space::ALL.map(Panels::first),
            was_running: false,
            net_menu: None,
            pcb: pcb::PcbView::default(),
            viewer3d: viewer3d::Viewer3d::default(),
            netlist: None,
            component_query: String::new(),
            code_tabs: Vec::new(),
            active_code: None,
            live_press: false,
            live_held: None,
            scopes: std::collections::BTreeMap::new(),
            live_speed: None,
            drop_preview: false,
        };
        if let Some(state) = &cc.wgpu_render_state {
            crate::render3d::Renderer3d::install(state);
            app.viewer3d.gpu = true;
        }
        if let Some(path) = args.project {
            app.open(&path);
        }
        if let Some(ws) = &mut app.workspace {
            if let Some(cell) = args.cell {
                match cell.parse::<circuiter_model::CellRef>() {
                    Ok(r) => match ws.design.resolve(&r, ws.project.name()) {
                        Some((id, _)) => ws.open_cell(id),
                        None => app.error = Some(format!("cell `{cell}` not found")),
                    },
                    Err(e) => app.error = Some(e),
                }
            }
            match args.simulate.as_deref() {
                Some("op") => ws.simulate(Analysis::Op),
                Some(t) if t == "tran" || t.starts_with("tran:") => {
                    let (mut tstop, mut tstep) = ws.default_tran();
                    if let Some(stop) = t.strip_prefix("tran:") {
                        match circuiter_model::units::parse_quantity(stop, Some("s")) {
                            Ok(v) => (tstop, tstep) = (v, v / 500.0),
                            Err(e) => app.error = Some(e.to_string()),
                        }
                    }
                    ws.simulate(Analysis::Tran { tstop, tstep, power_on: true });
                }
                Some("ac") => ws.simulate(Analysis::Ac { fstart: 1.0, fstop: 1e9, per_decade: 20 }),
                Some(other) => {
                    app.error = Some(format!("unknown analysis `{other}`; use op, tran or ac"))
                }
                None => {}
            }
            if args.view.as_deref() == Some("symbol") {
                ws.set_mode(Mode::Symbol);
            }
            if args.view.as_deref() == Some("pcb") {
                app.show_pcb = true;
            }
            if args.view.as_deref() == Some("diagram") {
                app.show_diagram = true;
            }
            if args.view.as_deref() == Some("board3d") {
                app.show_3d = true;
                app.viewer3d.board = true;
                // How to show the case: closed, open, section or none.
                app.viewer3d.case_view = match args.select.as_deref() {
                    Some("open") => viewer3d::CaseView::Open,
                    Some("section") => viewer3d::CaseView::Section,
                    Some("none") => viewer3d::CaseView::Hidden,
                    _ => viewer3d::CaseView::Closed,
                };
            }
            if args.view.as_deref() == Some("3d") {
                app.show_3d = true;
                // A feature of the body to open in the editor.
                if let Some(name) = &args.select {
                    let def = ws.current_def().and_then(|d| d.model3d.as_ref());
                    app.viewer3d.editor.selected =
                        def.and_then(|m| m.features.iter().position(|f| f.name == *name));
                }
            }
            if let Some(name) = args.select {
                let found = match ws.mode {
                    Mode::Schematic => ws
                        .schematic()
                        .and_then(|s| s.instances.iter().position(|i| i.name == name))
                        .map(Owner::Instance),
                    Mode::Symbol => ws
                        .symbol()
                        .and_then(|s| s.pins.iter().position(|p| p.port == name))
                        .map(Owner::Pin),
                };
                ws.select_only(found);
            }
        }
        if let Some(program) = args.program {
            app.open_code(&program);
        }
        if args.live {
            app.live_action(chrome::LiveAction::Start);
        }
        if args.results.as_deref() == Some("none") {
            app.show_bottom = false;
        }
        // Under the board: the rules check, nets, net classes or the stack-up.
        app.pcb.bottom_tab = match args.results.as_deref() {
            Some("nets") => 1,
            Some("classes") => 2,
            Some("pairs") => 3,
            Some("stackup") => 4,
            _ => 0,
        };
        if let Some(tab) = args.results.as_deref() {
            app.bottom_tab = match tab {
                "thermal" => 1,
                "problems" => 2,
                "oscilloscope" | "scope" => 3,
                _ => 0,
            };
        }
        // Opened running: the Simulation workspace, with the panels asked for.
        app.simulating = args.simulate.is_some() || args.live;
        app.was_running = app.simulating;
        if args.results.is_none() && app.simulating {
            app.show_bottom = true;
        }
        app.space = app.current_space();
        app
    }

    fn dirty(&self) -> bool {
        self.workspace.as_ref().is_some_and(|ws| ws.dirty) || self.code_dirty()
    }

    fn open(&mut self, path: &Path) {
        match Workspace::open(path) {
            Ok(ws) => {
                self.workspace = Some(ws);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Runs `action`, first asking what to do with unsaved changes if there are any.
    fn guarded(&mut self, ctx: &egui::Context, action: Pending) {
        if self.dirty() {
            self.prompt = Some(Prompt::Discard { then: action });
        } else {
            self.run(ctx, action);
        }
    }

    fn run(&mut self, ctx: &egui::Context, action: Pending) {
        match action {
            Pending::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Pending::Reload => self.reload(),
            Pending::OpenDialog => {
                if let Some(dir) =
                    rfd::FileDialog::new().set_title("Open Circuiter project").pick_folder()
                {
                    self.open(&dir);
                }
            }
            Pending::NewProjectDialog => {
                let picked = rfd::FileDialog::new()
                    .set_title("New project: choose or create an empty folder")
                    .pick_folder();
                if let Some(dir) = picked {
                    match Workspace::create(&dir) {
                        Ok(ws) => {
                            self.workspace = Some(ws);
                            self.error = None;
                        }
                        Err(e) => self.error = Some(format!("{e:#}")),
                    }
                }
            }
        }
    }

    fn reload(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let root = ws.project.root.clone();
        let path: Vec<(CellId, Option<String>)> =
            ws.levels.iter().map(|l| (l.cell.clone(), l.via.clone())).collect();
        self.open(&root);
        // Restore the hierarchy path as far as it still exists.
        let Some(ws) = &mut self.workspace else { return };
        let Some((first, _)) = path.first() else { return };
        if ws.design.cell(first).is_none() {
            return;
        }
        ws.open_cell(first.clone());
        for (_, via) in path.iter().skip(1) {
            let found = via
                .as_ref()
                .and_then(|name| ws.schematic()?.instances.iter().position(|i| &i.name == name));
            match found {
                Some(i) => ws.descend(i),
                None => break,
            }
        }
    }

    /// Takes in changes another program made to the open project's files.
    fn take_disk_changes(&mut self, ctx: &egui::Context) {
        let Some(ws) = &mut self.workspace else {
            self.watch = None;
            return;
        };
        let paths = ws.watched_paths();
        if !self.watch.as_ref().is_some_and(|w| w.watches(&paths)) {
            self.watch = Some(crate::watch::Watch::start(paths, ctx.clone()));
            return;
        }
        let Some(watch) = &self.watch else { return };
        if !watch.take() {
            return;
        }
        match ws.merge_from_disk() {
            Ok(merged) => {
                self.disk_failures = 0;
                if !merged.is_empty() {
                    let line = merged.describe();
                    self.pcb.note = Some(line.clone());
                    ws.notice = Some(line);
                }
            }
            Err(e) => {
                self.disk_failures += 1;
                if self.disk_failures >= 3 {
                    self.error = Some(format!("reading the project's files: {e:#}"));
                } else {
                    watch.retry();
                }
            }
        }
    }

    fn save(&mut self) {
        self.save_code();
        if let Some(ws) = &mut self.workspace {
            if let Err(e) = ws.save() {
                self.error = Some(format!("saving: {e:#}"));
            }
        }
    }

    fn simulate(&mut self, analysis: Analysis) {
        // Programs are read from disk when the simulation starts.
        self.save_code();
        if let Some(ws) = &mut self.workspace {
            ws.simulate(analysis);
            self.plot = PlotState::default();
        }
    }

    fn transient_prompt(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let (stop, step) = match ws.sim.as_ref().and_then(|s| s.results.as_ref()) {
            Some(Results::Tran { tstop, tstep, .. }) => (*tstop, *tstep),
            _ => ws.default_tran(),
        };
        self.prompt = Some(Prompt::Transient {
            stop: show(stop, "s"),
            step: show(step, "s"),
            power_on: true,
            error: None,
        });
    }

    fn import_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import SPICE models")
            .add_filter(
                "SPICE models",
                &["lib", "mod", "sub", "cir", "sp", "spi", "spice", "txt", "inc"],
            )
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                // Vendor files are often Latin-1; their models are ASCII either way.
                let text = String::from_utf8_lossy(&bytes);
                let defs = circuiter_sim::parse::definitions(&text);
                if defs.is_empty() {
                    self.error =
                        Some(format!("{} has no .subckt or .model definitions", path.display()));
                    return;
                }
                let selected = vec![true; defs.len()];
                let file =
                    path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                self.prompt = Some(Prompt::Import { file, defs, selected, outcome: None });
            }
            Err(e) => self.error = Some(format!("reading {}: {e}", path.display())),
        }
    }

    fn export_svg(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let name = format!("{}.svg", ws.current().cell.cell);
        if let Some(path) =
            rfd::FileDialog::new().set_file_name(name).add_filter("SVG", &["svg"]).save_file()
        {
            if let Err(e) = std::fs::write(&path, to_svg(&ws.scene, self.theme)) {
                self.error = Some(format!("writing {}: {e}", path.display()));
            }
        }
    }

    /// The sheet's parts and nets as a KiCad netlist, to lay the board out in KiCad.
    fn export_kicad_netlist(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let cell = ws.current().cell.clone();
        let text = match circuiter_pcb::kicad::netlist(&ws.design, &cell, ws.project.name()) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let name = format!("{}.net", cell.cell);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter("KiCad netlist", &["net"])
            .save_file()
        {
            if let Err(e) = std::fs::write(&path, text) {
                self.error = Some(format!("writing {}: {e}", path.display()));
            }
        }
    }

    /// The sheet's bill of materials as CSV: parts grouped by value and footprint.
    fn export_bom(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let cell = ws.current().cell.clone();
        let board = ws
            .current_def()
            .and_then(|d| d.board.clone())
            .unwrap_or_else(|| circuiter_model::board::Board::new(1.0, 1.0));
        let text = match circuiter_pcb::layout::Layout::new(&ws.design, &cell, &board) {
            Ok(layout) => circuiter_pcb::fab::bom(&layout),
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let name = format!("{}-bom.csv", cell.cell);
        if let Some(path) =
            rfd::FileDialog::new().set_file_name(name).add_filter("CSV", &["csv"]).save_file()
        {
            if let Err(e) = std::fs::write(&path, text) {
                self.error = Some(format!("writing {}: {e}", path.display()));
            }
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme == Some(self.theme.name) {
            return;
        }
        style::apply(ctx, self.theme.dark);
        self.applied_theme = Some(self.theme.name);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.workspace {
            Some(ws) => {
                let dot = if ws.dirty { " ●" } else { "" };
                format!("{}{dot} — Circuiter", ws.project.name())
            }
            None => "Circuiter".to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.prompt = Some(Prompt::Discard { then: Pending::Quit });
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        // Wait for a simulation to finish before counting frames.
        let busy = self
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.sim.as_ref().is_some_and(|s| s.job.is_some()));
        if !busy {
            self.frames += 1;
        }
        ctx.request_repaint();
        // A live run gets a second and a half to show something.
        let live = self.workspace.as_ref().is_some_and(|ws| ws.live.is_some());
        if live && self.frames < 10 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if self.frames == 10 {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            let saved = image::save_buffer(
                &path,
                &bytes,
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            );
            if let Err(e) = saved {
                eprintln!("error: saving screenshot to {}: {e}", path.display());
            }
            self.screenshot = None;
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Saving works while typing in the code editor too.
        if self.prompt.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S))
        {
            self.save();
        }
        // The footprint editor takes the keys while it's open.
        if self.prompt.is_some()
            || ctx.egui_wants_keyboard_input()
            || self.footprint_editor.is_open()
        {
            return;
        }
        let keys = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            Keys {
                open: cmd && i.key_pressed(Key::O),
                save: cmd && i.key_pressed(Key::S),
                new_cell: cmd && i.key_pressed(Key::N),
                undo: cmd && !shift && i.key_pressed(Key::Z),
                run: cmd && i.key_pressed(Key::R),
                redo: cmd && (i.key_pressed(Key::Y) || (shift && i.key_pressed(Key::Z))),
                reload: i.key_pressed(Key::F5),
                op: i.key_pressed(Key::F9),
                tran: i.key_pressed(Key::F10),
                fit: !cmd && (i.key_pressed(Key::F) || i.key_pressed(Key::Home)),
                up: i.modifiers.alt && i.key_pressed(Key::ArrowUp),
                back: (i.modifiers.alt && i.key_pressed(Key::ArrowLeft))
                    || i.pointer.button_pressed(egui::PointerButton::Extra1),
                forward: (i.modifiers.alt && i.key_pressed(Key::ArrowRight))
                    || i.pointer.button_pressed(egui::PointerButton::Extra2),
                escape: i.key_pressed(Key::Escape),
                delete: i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                rotate: !cmd && i.key_pressed(Key::R),
                mirror: !cmd && i.key_pressed(Key::X),
                wire: !cmd && i.key_pressed(Key::W),
                hand: !cmd && i.key_pressed(Key::H),
                label: !cmd && i.key_pressed(Key::L),
                port: !cmd && i.key_pressed(Key::P),
                add: !cmd && i.key_pressed(Key::A),
                search: cmd && i.key_pressed(Key::K),
                flip: i.key_pressed(Key::Slash),
                view: !cmd && !shift && i.key_pressed(Key::V),
                ground: !cmd && !shift && i.key_pressed(Key::G),
                duplicate: cmd && i.key_pressed(Key::D),
                vdd: !cmd && shift && i.key_pressed(Key::V),
                rect: !cmd && i.key_pressed(Key::B),
                circle: !cmd && i.key_pressed(Key::C),
                text: !cmd && i.key_pressed(Key::T),
                no_connect: !cmd && i.key_pressed(Key::Q),
            }
        });
        if keys.open {
            self.guarded(ctx, Pending::OpenDialog);
        }
        // In the board view F5 shows net colours, as in Altium.
        if keys.reload && !self.show_pcb {
            self.guarded(ctx, Pending::Reload);
        }
        if keys.op {
            self.simulate(Analysis::Op);
        }
        if keys.run {
            if let Some(ws) = &self.workspace {
                let a = ws.run_analysis();
                self.simulate(a);
            }
        }
        if keys.tran {
            self.transient_prompt();
        }
        if keys.save {
            self.save();
        }
        let Some(ws) = &mut self.workspace else { return };
        if keys.search && ws.editable() && ws.mode == Mode::Schematic {
            self.prompt = Some(Prompt::AddPart { query: String::new() });
        }
        if keys.new_cell {
            self.prompt = Some(new_cell_prompt(ws, false));
        }
        if keys.fit {
            ws.levels.last_mut().expect("level").camera = None;
        }
        if keys.up && ws.levels.len() > 1 {
            ws.ascend_to(ws.levels.len() - 1);
        }
        if keys.back {
            ws.go_back();
        }
        if keys.forward {
            ws.go_forward();
        }
        if keys.escape {
            match ws.sim.as_mut().filter(|s| s.job.is_some()) {
                Some(sim) => sim.cancel(),
                None => ws.escape(),
            }
        }
        if keys.undo {
            ws.undo();
        }
        if keys.redo {
            ws.redo();
        }
        // The board and 3D views take their own keys.
        if self.show_pcb || self.show_3d {
            return;
        }
        if keys.view {
            ws.set_mode(match ws.mode {
                Mode::Schematic => Mode::Symbol,
                Mode::Symbol => Mode::Schematic,
            });
        }
        if !ws.editable() {
            return;
        }
        if keys.delete && !ws.selection.is_empty() {
            ws.delete_selection();
        }
        if keys.rotate {
            ws.rotate();
        }
        if keys.mirror {
            ws.mirror();
        }
        if ws.mode == Mode::Symbol {
            if keys.label {
                ws.tool = Tool::Line { points: Vec::new() };
            }
            if keys.rect {
                ws.tool = Tool::Rect { from: None };
            }
            if keys.circle {
                ws.tool = Tool::Circle { center: None };
            }
            if keys.add {
                ws.tool = Tool::Arc { center: None, start: None };
            }
            if keys.text {
                self.prompt = Some(Prompt::SymbolText { text: String::new() });
            }
            if keys.port {
                if let Err(e) = ws.start_pin(None) {
                    self.error = Some(e);
                }
            }
            return;
        }
        if keys.flip {
            ws.flip_wire_corner();
        }
        if keys.wire {
            ws.tool = Tool::Wire { points: Vec::new(), horizontal_first: true };
        }
        if keys.no_connect {
            ws.tool = Tool::NoConnect;
        }
        if keys.rect {
            ws.tool = Tool::GroupBox { from: None };
        }
        if keys.text {
            self.prompt = Some(Prompt::Note { text: String::new() });
        }
        if keys.hand {
            ws.tool = if ws.tool == Tool::Pan { Tool::Select } else { Tool::Pan };
        }
        if keys.label {
            self.prompt = Some(Prompt::Label { net: String::new(), global: false });
        }
        if keys.port {
            self.prompt = Some(new_port_prompt(true));
        }
        if keys.add {
            self.prompt = Some(Prompt::AddPart { query: String::new() });
        }
        if keys.duplicate {
            if let Err(e) = ws.duplicate() {
                self.error = Some(e);
            }
        }
        // Copy, cut and paste arrive as events, not keys (egui takes them from the
        // platform's shortcuts).
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::Copy => {
                    if let Some(text) = ws.copy() {
                        ctx.copy_text(text);
                    }
                }
                egui::Event::Cut => {
                    if let Some(text) = ws.cut() {
                        ctx.copy_text(text);
                    }
                }
                egui::Event::Paste(text) if ws.editable() => {
                    if let Err(e) = ws.paste(Some(&text), self.cursor) {
                        self.error = Some(e);
                    }
                }
                _ => {}
            }
        }
        if keys.ground {
            ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "gnd"));
        }
        if keys.vdd {
            ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "vdd"));
        }
    }

    /// A part dragged from the library or the strip: previewed under the pointer while
    /// over the sheet, placed where it is let go.
    fn drop_part(&mut self, ctx: &egui::Context, cursor: Option<Pt>, tolerance: f64) {
        let Some(ws) = &mut self.workspace else { return };
        let dragged = egui::DragAndDrop::payload::<DraggedCell>(ctx);
        match (dragged, cursor) {
            (Some(d), Some(at)) if ws.placeable(&d.0) => {
                if !self.drop_preview || !matches!(ws.tool, Tool::Place { .. }) {
                    ws.start_placing(&d.0);
                    self.drop_preview = true;
                }
                if ctx.input(|i| i.pointer.any_released()) {
                    egui::DragAndDrop::clear_payload(ctx);
                    self.drop_preview = false;
                    let placed = ws.gesture(canvas::Gesture::Click { at, shift: false }, tolerance);
                    if let Some(e) = placed {
                        self.error = Some(e);
                    }
                }
            }
            _ if self.drop_preview => {
                // Dragged off the sheet again, or let go elsewhere.
                self.drop_preview = false;
                if matches!(ws.tool, Tool::Place { .. }) {
                    ws.tool = Tool::Select;
                }
            }
            _ => {}
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            let has_ws = self.workspace.is_some();
            ui.menu_button("File", |ui| {
                if ui.button("New Project…").clicked() {
                    self.guarded(&ctx, Pending::NewProjectDialog);
                }
                if ui.button("Open Project…   Ctrl+O").clicked() {
                    self.guarded(&ctx, Pending::OpenDialog);
                }
                if ui.add_enabled(has_ws, egui::Button::new("New Cell…   Ctrl+N")).clicked() {
                    self.prompt = self.workspace.as_ref().map(|ws| new_cell_prompt(ws, false));
                }
                if ui.add_enabled(has_ws, egui::Button::new("New 3D Part…")).clicked() {
                    if let Some(ws) = &self.workspace {
                        self.prompt = Some(new_cell_prompt(ws, true));
                    }
                    ui.close();
                }
                if ui.add_enabled(self.dirty(), egui::Button::new("Save   Ctrl+S")).clicked() {
                    self.save();
                }
                if ui.add_enabled(has_ws, egui::Button::new("Reload   F5")).clicked() {
                    self.guarded(&ctx, Pending::Reload);
                }
                ui.separator();
                if ui.add_enabled(has_ws, egui::Button::new("Import SPICE Models…")).clicked() {
                    self.import_dialog();
                }
                if ui.add_enabled(has_ws, egui::Button::new("Export SVG…")).clicked() {
                    self.export_svg();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    self.guarded(&ctx, Pending::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                let Some(ws) = &mut self.workspace else { return };
                if ui.add_enabled(ws.can_undo(), egui::Button::new("Undo   Ctrl+Z")).clicked() {
                    ws.undo();
                }
                if ui.add_enabled(ws.can_redo(), egui::Button::new("Redo   Ctrl+Y")).clicked() {
                    ws.redo();
                }
                ui.separator();
                let editable = ws.editable();
                let has_sel = !ws.selection.is_empty();
                if ui.add_enabled(editable && has_sel, egui::Button::new("Rotate   R")).clicked() {
                    ws.rotate();
                }
                if ui.add_enabled(editable && has_sel, egui::Button::new("Mirror   X")).clicked() {
                    ws.mirror();
                }
                if ui.add_enabled(editable && has_sel, egui::Button::new("Delete   Del")).clicked()
                {
                    ws.delete_selection();
                }
                ui.separator();
                let sheet = ws.mode == Mode::Schematic;
                if ui
                    .add_enabled(editable && sheet && has_sel, egui::Button::new("Cut   Ctrl+X"))
                    .clicked()
                {
                    if let Some(text) = ws.cut() {
                        ui.ctx().copy_text(text);
                    }
                }
                if ui.add_enabled(sheet && has_sel, egui::Button::new("Copy   Ctrl+C")).clicked() {
                    if let Some(text) = ws.copy() {
                        ui.ctx().copy_text(text);
                    }
                }
                if ui.add_enabled(editable && sheet, egui::Button::new("Paste   Ctrl+V")).clicked()
                {
                    if let Err(e) = ws.paste(None, None) {
                        self.error = Some(e);
                    }
                }
                if ui
                    .add_enabled(
                        editable && sheet && has_sel,
                        egui::Button::new("Duplicate   Ctrl+D"),
                    )
                    .clicked()
                {
                    if let Err(e) = ws.duplicate() {
                        self.error = Some(e);
                    }
                }
                ui.separator();
                let parts = ws.selection.iter().any(|o| matches!(o, Owner::Instance(_)));
                if ui
                    .add_enabled(
                        sheet && parts,
                        egui::Button::new("Make Component from Selection…"),
                    )
                    .clicked()
                {
                    self.prompt = Some(make_component_prompt(ws));
                }
            });
            ui.menu_button("Place", |ui| {
                let Some(ws) = &mut self.workspace else { return };
                let sheet = ws.editable() && ws.mode == Mode::Schematic;
                if ui.add_enabled(sheet, egui::Button::new("Part…   A")).clicked() {
                    self.prompt = Some(Prompt::AddPart { query: String::new() });
                }
                if ui.add_enabled(sheet, egui::Button::new("Wire   W")).clicked() {
                    ws.tool = Tool::Wire { points: Vec::new(), horizontal_first: true };
                }
                if ui.add_enabled(sheet, egui::Button::new("Net Label…   L")).clicked() {
                    self.prompt = Some(Prompt::Label { net: String::new(), global: false });
                }
                if ui.add_enabled(sheet, egui::Button::new("Global Label…")).clicked() {
                    self.prompt = Some(Prompt::Label { net: String::new(), global: true });
                }
                if ui.add_enabled(sheet, egui::Button::new("Port…   P")).clicked() {
                    self.prompt = Some(new_port_prompt(true));
                }
                if ui.add_enabled(sheet, egui::Button::new("No-Connect Flag   Q")).clicked() {
                    ws.tool = Tool::NoConnect;
                }
                ui.separator();
                if ui.add_enabled(sheet, egui::Button::new("Ground   G")).clicked() {
                    ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "gnd"));
                }
                if ui.add_enabled(sheet, egui::Button::new("VDD   Shift+V")).clicked() {
                    ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "vdd"));
                }
                if ui
                    .add_enabled(sheet, egui::Button::new("Power Rail (+5V, +3V3, -12V…)"))
                    .clicked()
                {
                    ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "rail"));
                }
                if ui
                    .add_enabled(sheet, egui::Button::new("Power Flag"))
                    .on_hover_text(
                        "Tells the rules check a net is supplied through something it can't follow",
                    )
                    .clicked()
                {
                    ws.start_placing(&CellId::new(circuiter_model::STD_LIBRARY, "pwr_flag"));
                }
            });
            ui.menu_button("Tools", |ui| {
                let Some(ws) = &mut self.workspace else { return };
                let sheet = ws.has_schematic();
                let editable = ws.editable();
                if ui
                    .add_enabled(sheet && editable, egui::Button::new("Annotate Schematic…"))
                    .clicked()
                {
                    self.prompt = Some(Prompt::Annotate { columns: false, reset: false });
                }
                if ui
                    .add_enabled(sheet, egui::Button::new("Electrical Rules Check"))
                    .on_hover_text("What the pins on each net say about it, in Problems")
                    .clicked()
                {
                    (self.show_pcb, self.show_3d, self.show_netlist) = (false, false, false);
                    self.show_diagram = false;
                    self.show_bottom = true;
                    self.bottom_tab = 2;
                }
                ui.separator();
                if ui.add_enabled(sheet, egui::Button::new("Board (PCB)")).clicked() {
                    (self.show_pcb, self.show_3d, self.show_netlist) = (true, false, false);
                    self.show_diagram = false;
                }
                if ui
                    .add_enabled(sheet && editable, egui::Button::new("Update PCB from Schematic"))
                    .clicked()
                {
                    (self.show_pcb, self.show_3d, self.show_netlist) = (true, false, false);
                    self.show_diagram = false;
                    self.pcb_update_from_schematic();
                }
                ui.separator();
                if ui.add_enabled(sheet, egui::Button::new("Export KiCad Netlist…")).clicked() {
                    self.export_kicad_netlist();
                }
                if ui.add_enabled(sheet, egui::Button::new("Export Bill of Materials…")).clicked()
                {
                    self.export_bom();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit to Window   F").clicked() {
                    if let Some(ws) = &mut self.workspace {
                        ws.levels.last_mut().expect("level").camera = None;
                    }
                }
                if let Some(ws) = &mut self.workspace {
                    let has_schematic = ws.has_schematic();
                    if ui
                        .add_enabled(
                            has_schematic,
                            egui::Button::selectable(ws.mode == Mode::Schematic, "Schematic   V"),
                        )
                        .clicked()
                    {
                        ws.set_mode(Mode::Schematic);
                    }
                    if ui
                        .add(egui::Button::selectable(ws.mode == Mode::Symbol, "Symbol   V"))
                        .clicked()
                    {
                        ws.set_mode(Mode::Symbol);
                    }
                    ui.separator();
                }
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.separator();
                for theme in Theme::ALL {
                    if ui.radio(std::ptr::eq(self.theme, theme), theme.name).clicked() {
                        self.theme = theme;
                    }
                }
            });
            ui.menu_button("Simulate", |ui| {
                let has_ws = self.workspace.is_some();
                if ui.add_enabled(has_ws, egui::Button::new("Operating Point   F9")).clicked() {
                    self.simulate(Analysis::Op);
                }
                if ui.add_enabled(has_ws, egui::Button::new("Transient…   F10")).clicked() {
                    self.transient_prompt();
                }
                if ui.add_enabled(has_ws, egui::Button::new("AC Sweep…")).clicked() {
                    self.prompt = Some(ac_prompt());
                }
                ui.separator();
                let has_sim = self.workspace.as_ref().is_some_and(|w| w.sim.is_some());
                if ui.add_enabled(has_sim, egui::Button::new("Clear Results")).clicked() {
                    if let Some(ws) = &mut self.workspace {
                        ws.sim = None;
                        ws.sim_error = None;
                    }
                }
            });
            ui.menu_button("Navigate", |ui| {
                let Some(ws) = &mut self.workspace else { return };
                let nested = ws.levels.len() > 1;
                if ui.add_enabled(nested, egui::Button::new("Up One Level   Alt+Up")).clicked() {
                    ws.ascend_to(ws.levels.len() - 1);
                }
                if ui.add_enabled(nested, egui::Button::new("Top Level")).clicked() {
                    ws.ascend_to(1);
                }
                ui.separator();
                let back = ws.back_to().is_some();
                if ui.add_enabled(back, egui::Button::new("Back   Alt+Left")).clicked() {
                    ws.go_back();
                }
                let forward = ws.forward_to().is_some();
                if ui.add_enabled(forward, egui::Button::new("Forward   Alt+Right")).clicked() {
                    ws.go_forward();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard Shortcuts").clicked() {
                    self.prompt = Some(Prompt::Shortcuts);
                }
            });
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        if self.show_pcb {
            pcb::board_inspector(self, ui);
            return;
        }
        let Some(ws) = &mut self.workspace else { return };
        ui.add_space(4.0);
        let field = &mut self.field;
        let mut prompt = None;
        let mut open_3d = false;
        let mut edit_footprint = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let mut sel = ws.selection.iter();
            match (sel.next().copied(), sel.next()) {
                (Some(Owner::Instance(i)), None) => instance_panel(ui, ws, i, field),
                (Some(Owner::Pin(i)), None) => pin_panel(ui, ws, i, field),
                (Some(Owner::Graphic(i)), None) => graphic_panel(ui, ws, i, field),
                (Some(Owner::Note(i)), None) => note_panel(ui, ws, i, field),
                (Some(Owner::Wire(_) | Owner::Label(_) | Owner::Port(_)), None) => {
                    if let Some(n) = ws.net {
                        net_panel(ui, ws, n);
                    }
                }
                (Some(_), Some(_)) => {
                    let p = style::palette(ui);
                    ui.label(
                        RichText::new(format!("{} objects selected", ws.selection.len()))
                            .size(18.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    let parts = ws.selection.iter().any(|o| matches!(o, Owner::Instance(_)));
                    if parts && ws.mode == Mode::Schematic {
                        let make = egui::Button::new(format!(
                            "{}  Make Component from Selection…",
                            style::icon::PUZZLE_PIECE
                        ));
                        if ui.add(make).clicked() {
                            prompt = Some(make_component_prompt(ws));
                        }
                        ui.label(
                            RichText::new(
                                "Turns these parts, and the wires between them, into a new \
                                 part you can place anywhere: each connection they have to the \
                                 rest of the sheet becomes a port.",
                            )
                            .color(p.weak),
                        );
                        ui.add_space(6.0);
                    }
                    if ws.editable()
                        && ui.button(format!("{}  Delete", style::icon::TRASH)).clicked()
                    {
                        ws.delete_selection();
                    }
                }
                _ => cell_panel(ui, ws, field, &mut prompt, &mut open_3d, &mut edit_footprint),
            }
        });
        if prompt.is_some() {
            self.prompt = prompt;
        }
        if open_3d {
            self.open_body_3d();
        }
        if let (Some(view), Some(ws)) = (edit_footprint, &self.workspace) {
            self.footprint_editor.start(ws.current().cell.clone(), view);
        }
    }

    /// Opens `view` in the footprint editor, as Edit Pads does, for the cell in front.
    #[cfg(test)]
    pub fn edit_footprint(&mut self, view: circuiter_model::board::FootprintView) {
        let cell = self.workspace.as_ref().expect("open").current().cell.clone();
        self.footprint_editor.start(cell, view);
    }

    #[cfg(test)]
    pub fn footprint_editor_open(&self) -> bool {
        self.footprint_editor.is_open()
    }

    /// Whether the 3D view is showing.
    #[cfg(test)]
    pub fn showing_3d(&self) -> bool {
        self.show_3d
    }

    /// Whether the Diagram view is showing.
    #[cfg(test)]
    pub fn showing_diagram(&self) -> bool {
        self.show_diagram
    }

    /// Where part `name` is on screen in the Diagram view.
    #[cfg(test)]
    pub fn diagram_part(&self, name: &str) -> Option<egui::Pos2> {
        self.diagram.part_on_screen(name)
    }

    /// Shows the cell being edited in the 3D view with its Body panel, to model it.
    fn open_body_3d(&mut self) {
        self.show_3d = true;
        self.show_pcb = false;
        self.show_netlist = false;
        self.show_diagram = false;
        self.viewer3d.body_mode = true;
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        // The right-hand items first, so the hint on the left gets the room left over
        // and is cut short rather than drawn over them.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(ws) = &self.workspace {
                if let Some(sim) = &ws.sim {
                    let (text, c) = panels::sim_status(sim, style::palette(ui));
                    if !text.is_empty() {
                        ui.label(RichText::new(text).color(c).small());
                        ui.separator();
                    }
                }
                if let Some(cam) = ws.current().camera {
                    ui.label(RichText::new(format!("{:.1} px/mm", cam.scale)).monospace().weak());
                    ui.separator();
                }
                let problems = ws.problems();
                let errors = problems.iter().filter(|(s, _)| *s == Severity::Error).count();
                let icon = style::icon::CHECK_CIRCLE;
                let text = match (errors, problems.len()) {
                    (_, 0) => RichText::new(format!("{icon} no problems")).color(OK),
                    (0, n) => RichText::new(format!("{} {n} warning(s)", style::icon::WARNING))
                        .color(WARNING),
                    (e, n) => RichText::new(format!(
                        "{} {e} error(s), {} warning(s)",
                        style::icon::X_CIRCLE,
                        n - e
                    ))
                    .color(ERROR),
                };
                ui.label(text);
                if let Some(owner) = self.hovered.filter(|_| ws.editable()) {
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(ws.describe(owner)).weak()).truncate());
                }
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                match self.cursor {
                    Some(p) => {
                        let mil = |mm: f64| (mm / 0.0254).round() as i64;
                        ui.label(
                            RichText::new(format!(
                                "X {:>8.2}  Y {:>8.2} mm   ({}, {} mil)",
                                p.x,
                                p.y,
                                mil(p.x),
                                mil(p.y)
                            ))
                            .monospace(),
                        );
                    }
                    None => {
                        ui.label(RichText::new("X        –  Y        – mm").monospace().weak());
                    }
                }
                ui.separator();
                let Some(ws) = &self.workspace else { return };
                if self.show_pcb {
                    ui.label(RichText::new(self.pcb.tool.name()).strong());
                    ui.add(
                        egui::Label::new(RichText::new(pcb::hint(&self.pcb.tool)).weak())
                            .truncate(),
                    );
                } else if let Some(notice) = &ws.notice {
                    let text = RichText::new(format!("{}  {notice}", style::icon::LIGHTNING));
                    ui.add(egui::Label::new(text.color(ACCENT)).truncate());
                } else if ws.editable() {
                    ui.label(RichText::new(ws.tool.name()).strong());
                    ui.add(egui::Label::new(RichText::new(ws.hint()).weak()).truncate());
                } else if let Some(owner) = self.hovered {
                    ui.add(egui::Label::new(ws.describe(owner)).truncate());
                }
            });
        });
    }

    /// The results panel under the schematic: waveforms, or a Bode plot.
    fn results_panel(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let Some(sim) = &ws.sim else { return };
        let ac = matches!(sim.results, Some(Results::Ac { .. }));
        let has_heat = sim.monitor.as_ref().is_some_and(|m| !m.parts.is_empty());
        let heat = self.plot_heat && !ac;
        let mut remove = None;
        let p = style::palette(ui);
        ui.horizontal(|ui| {
            if heat {
                for e in sim.events() {
                    ui.colored_label(
                        p.error,
                        format!("{}  {} at {}", style::icon::FIRE, e.path, show(e.time, "s")),
                    )
                    .on_hover_text(e.describe());
                }
                if !has_heat {
                    ui.label(RichText::new("No part warmed up.").color(p.weak));
                }
            } else {
                if sim.stale {
                    ui.colored_label(p.warning, "Out of date: run the simulation again");
                }
                for (k, &probe) in sim.probes.iter().enumerate() {
                    let name = &sim.netlist.nets[probe].name;
                    let chip = egui::Button::new(
                        RichText::new(format!(
                            "{}  {name}   {}",
                            style::icon::SQUARE,
                            style::icon::X
                        ))
                        .color(plot::trace_color(k)),
                    )
                    .frame(false);
                    if ui.add(chip).on_hover_text("Remove from the plot").clicked() {
                        remove = Some(k);
                    }
                }
                if sim.probes.is_empty() {
                    ui.label(RichText::new("Select a wire and choose Add to Plot").color(p.weak));
                }
            }
            style::trailing(ui, |ui| {
                ui.label(
                    RichText::new("wheel zooms · drag pans · double-click shows all")
                        .small()
                        .color(p.weak),
                );
            });
        });
        let area = ui.available_rect_before_wrap();
        let names: Vec<String> =
            sim.probes.iter().map(|&p| sim.netlist.nets[p].name.clone()).collect();
        if let (true, Some(m), Some(Results::Tran { result, tstop, .. })) =
            (heat, &sim.monitor, &sim.results)
        {
            // One trace per part that warmed by more than a degree, and the air in the
            // case when it's in one.
            let mut series: Vec<(String, Vec<f64>)> = m
                .parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.peak > m.ambient + 1.0)
                .map(|(k, p)| {
                    let temps: Vec<f64> =
                        m.temperatures.iter().map(|row| f64::from(row[k])).collect();
                    (p.path.clone(), temps)
                })
                .collect();
            if !m.air.is_empty() {
                series.push((
                    "air in the case".to_owned(),
                    m.air.iter().map(|t| f64::from(*t)).collect(),
                ));
            }
            let n = result.time.len();
            let traces: Vec<Trace<'_>> = series
                .iter()
                .map(|(name, t)| Trace {
                    name,
                    x: &result.time[..n.min(t.len())],
                    y: &t[..n.min(t.len())],
                })
                .collect();
            let axis = XAxis { unit: "s", range: (0.0, *tstop), log: false };
            plot::show_lanes(ui, area, &mut self.plot, &traces, axis, "°C", self.theme);
            return;
        }
        match &sim.results {
            Some(Results::Mixed { logic, tstop, .. }) => {
                // Analog signals as waveforms above, digital ones as logic lanes below.
                type Wave<'a> = (&'a str, (&'a [f64], Vec<f64>));
                let waves: Vec<Wave<'_>> = sim
                    .probes
                    .iter()
                    .zip(&names)
                    .filter_map(|(&p, n)| Some((n.as_str(), sim.waveform(p)?)))
                    .collect();
                let bits: Vec<plot::LogicTrace<'_>> = sim
                    .probes
                    .iter()
                    .zip(&names)
                    .filter(|(p, _)| sim.waveform(**p).is_none())
                    .filter_map(|(&p, n)| {
                        Some(plot::LogicTrace {
                            name: n,
                            changes: logic.history.get(p)?,
                            width: sim.netlist.nets.get(p)?.width,
                        })
                    })
                    .collect();
                let traces: Vec<Trace<'_>> =
                    waves.iter().map(|(n, (t, v))| Trace { name: n, x: t, y: v }).collect();
                let axis = XAxis { unit: "s", range: (0.0, *tstop), log: false };
                let share = match (traces.is_empty(), bits.is_empty()) {
                    (false, true) => 1.0,
                    (true, false) => 0.0,
                    _ => traces.len() as f32 / (traces.len() + bits.len()).max(1) as f32,
                };
                let (top, bottom) = area.split_top_bottom_at_fraction(share.clamp(0.0, 1.0));
                if !traces.is_empty() {
                    plot::show_lanes(ui, top, &mut self.plot, &traces, axis, "V", self.theme);
                }
                if !bits.is_empty() {
                    plot::show_logic(ui, bottom, &mut self.plot, &bits, *tstop, self.theme);
                }
            }
            Some(Results::Logic(run)) => {
                let traces: Vec<plot::LogicTrace<'_>> = sim
                    .probes
                    .iter()
                    .zip(&names)
                    .filter_map(|(&p, n)| {
                        Some(plot::LogicTrace {
                            name: n,
                            changes: run.history.get(p)?,
                            width: sim.netlist.nets.get(p)?.width,
                        })
                    })
                    .collect();
                plot::show_logic(ui, area, &mut self.plot, &traces, run.end.max(1e-12), self.theme);
            }
            Some(Results::Tran { tstop, .. }) => {
                let waves: Vec<(&[f64], Vec<f64>)> =
                    sim.probes.iter().filter_map(|&p| sim.waveform(p)).collect();
                let traces: Vec<Trace<'_>> = waves
                    .iter()
                    .zip(&names)
                    .map(|((t, v), n)| Trace { name: n, x: t, y: v })
                    .collect();
                let axis = XAxis { unit: "s", range: (0.0, *tstop), log: false };
                plot::show_lanes(ui, area, &mut self.plot, &traces, axis, "V", self.theme);
            }
            Some(Results::Ac { fstart, fstop, .. }) => {
                let responses: Vec<(&[f64], Vec<f64>, Vec<f64>)> = sim
                    .probes
                    .iter()
                    .filter_map(|&p| {
                        let (f, v) = sim.response(p)?;
                        let db = v.iter().map(|c| c.db()).collect();
                        // Without signal the phase is noise; leave it out.
                        let mut phase: Vec<f64> = v
                            .iter()
                            .map(|c| if c.abs() > 1e-15 { c.phase() } else { f64::NAN })
                            .collect();
                        plot::unwrap_phase(&mut phase);
                        Some((f, db, phase))
                    })
                    .collect();
                let axis = XAxis { unit: "Hz", range: (*fstart, *fstop), log: true };
                let mag: Vec<Trace<'_>> = responses
                    .iter()
                    .zip(&names)
                    .map(|((f, db, _), n)| Trace { name: n, x: f, y: db })
                    .collect();
                let phase: Vec<Trace<'_>> = responses
                    .iter()
                    .zip(&names)
                    .map(|((f, _, ph), n)| Trace { name: n, x: f, y: ph })
                    .collect();
                let (top, bottom) = area.split_top_bottom_at_fraction(0.6);
                plot::show_plot(ui, top, &mut self.plot, &mag, axis, "dB", self.theme);
                plot::show_plot(ui, bottom, &mut self.plot, &phase, axis, "°", self.theme);
            }
            _ => {}
        }
        if let (Some(k), Some(sim)) = (remove, &mut ws.sim) {
            sim.probes.remove(k);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(RichText::new("Circuiter").size(40.0).strong());
            ui.label(RichText::new("Design, simulate and build hardware").weak());
            ui.add_space(24.0);
            let ctx = ui.ctx().clone();
            if ui.button(RichText::new("New Project…").size(16.0)).clicked() {
                self.run(&ctx, Pending::NewProjectDialog);
            }
            ui.add_space(6.0);
            if ui.button(RichText::new("Open Project…").size(16.0)).clicked() {
                self.run(&ctx, Pending::OpenDialog);
            }
            // Example projects, when run from a source checkout.
            let mut examples: Vec<PathBuf> = std::fs::read_dir("examples")
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(circuiter_model::project::MANIFEST_FILE).is_file())
                .collect();
            examples.sort();
            if !examples.is_empty() {
                ui.add_space(16.0);
                ui.label(RichText::new("Examples").weak());
            }
            for example in examples {
                let name = example.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let description = circuiter_model::Project::load(&example)
                    .map(|p| p.manifest.project.description)
                    .unwrap_or_default();
                if ui.link(name.replace('_', " ")).on_hover_text(description).clicked() {
                    self.open(&example);
                }
            }
        });
    }

    fn prompts(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.prompt else { return };
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let mut then: Option<Pending> = None;
        let mut save_first = false;
        let mut program: Option<String> = None;
        let mut open_3d = false;
        let mut failed: Option<String> = None;
        let ws = &mut self.workspace;

        egui::Modal::new(egui::Id::new("prompt")).show(ctx, |ui| {
            ui.set_width(380.0);
            match prompt {
                Prompt::AddPart { query } => {
                    ui.heading("Add Part");
                    let r = ui.add(
                        egui::TextEdit::singleline(query)
                            .hint_text("Search parts: resistor, nmos, clock…")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    let Some(ws) = ws else { return };
                    let q = query.to_lowercase();
                    // (cell, folder, description)
                    let mut matches: Vec<(CellId, String, String)> = Vec::new();
                    for lib in ws.design.libraries() {
                        for def in lib.cells.values() {
                            let id = CellId::new(&lib.name, &def.name);
                            let hay =
                                format!("{} {} {}", def.name, def.folder, def.cell.description)
                                    .to_lowercase();
                            if hay.contains(&q) && ws.placeable(&id) {
                                matches.push((id, def.folder.clone(), def.cell.description.clone()));
                            }
                        }
                    }
                    ui.add_space(4.0);
                    let mut pick = None;
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (i, (id, folder, desc)) in matches.iter().enumerate() {
                            let text = RichText::new(id.to_string()).monospace();
                            let text = if i == 0 { text.strong() } else { text };
                            ui.horizontal(|ui| {
                                if ui.selectable_label(i == 0, text).on_hover_text(desc).clicked() {
                                    pick = Some(id.clone());
                                }
                                if !folder.is_empty() {
                                    ui.label(RichText::new(format!("{folder}/")).weak().small());
                                }
                            });
                        }
                        if matches.is_empty() {
                            ui.label(RichText::new("No matching parts").weak());
                        }
                    });
                    if enter {
                        pick = pick.or_else(|| matches.first().map(|(id, ..)| id.clone()));
                    }
                    if let Some(id) = pick {
                        ws.start_placing(&id);
                        close = true;
                    }
                }
                Prompt::Annotate { columns, reset } => {
                    ui.heading("Annotate");
                    ui.label("Gives every part on this sheet its reference: its prefix and the next number, in reading order.");
                    ui.radio_value(columns, false, "Across rows, top to bottom");
                    ui.radio_value(columns, true, "Down columns, left to right");
                    ui.checkbox(reset, "Renumber every part (otherwise numbered parts keep theirs)");
                    ui.add_space(6.0);
                    if ui.button("Annotate").clicked() || enter {
                        if let Some(ws) = ws {
                            let order = if *columns {
                                circuiter_model::annotate::Order::Columns
                            } else {
                                circuiter_model::annotate::Order::Rows
                            };
                            match ws.annotate(order, *reset) {
                                Ok(0) => ws.notice = Some("Every part already has its reference".into()),
                                Ok(n) => ws.notice = Some(format!("Renamed {n} part(s); the board follows")),
                                Err(e) => failed = Some(e),
                            }
                        }
                        close = true;
                    }
                }
                Prompt::Label { net, global } => {
                    ui.heading("Net Label");
                    ui.label("Labels with the same name on a sheet are connected.");
                    let r = ui.add(egui::TextEdit::singleline(net).hint_text("VDD, CLK, DATA0…"));
                    r.request_focus();
                    ui.checkbox(global, "Global: joins this net on every sheet of the design");
                    if enter && !net.trim().is_empty() {
                        if let Some(ws) = ws {
                            ws.tool = Tool::Label { net: net.trim().to_owned(), global: *global };
                        }
                        close = true;
                    }
                }
                Prompt::Port { name, kind, place, error } => {
                    ui.heading("Port");
                    ui.label("A connection to this cell from the sheets that use it.");
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("in, out, vdd…"));
                    r.request_focus();
                    egui::ComboBox::from_label("Direction")
                        .selected_text(kind_label(*kind))
                        .show_ui(ui, |ui| {
                            for k in PORT_KINDS {
                                ui.selectable_value(kind, k, kind_label(k));
                            }
                        });
                    if *place {
                        ui.label(RichText::new("Then click where it connects.").weak());
                    }
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter && !name.trim().is_empty() {
                        if let Some(ws) = ws {
                            let name = name.trim().to_owned();
                            if *place {
                                ws.tool = Tool::Port { name, kind: *kind };
                                close = true;
                            } else {
                                let k = *kind;
                                match ws.edit(|_, _, d| interface::add_port(d, &name, k)) {
                                    Ok(()) => close = true,
                                    Err(e) => *error = Some(e),
                                }
                            }
                        }
                    }
                }
                Prompt::Transient { stop, step, power_on, error } => {
                    ui.heading("Transient Analysis");
                    ui.label("Simulates the circuit over time.");
                    egui::Grid::new("tran").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Stop time");
                        let r = ui.add(egui::TextEdit::singleline(stop).hint_text("30n, 2m…"));
                        if ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                        ui.end_row();
                        ui.label("Resolution").on_hover_text(
                            "The largest time step; it shrinks automatically on fast edges",
                        );
                        ui.add(egui::TextEdit::singleline(step).hint_text("50p, 1u…"));
                        ui.end_row();
                    });
                    ui.checkbox(power_on, "Start from power-on").on_hover_text(
                        "On: everything starts off and the sources switch on at 0 s, as when \
                         you turn a real circuit on. Off: start from the steady state, as if it \
                         had been on forever (SPICE's usual start).",
                    );
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    ui.add_space(4.0);
                    let run = ui.button("Run").clicked();
                    if enter || run {
                        let parse = |t: &str| circuiter_model::units::parse_quantity(t, Some("s"));
                        match (parse(stop), parse(step)) {
                            (Ok(a), Ok(b)) if a > 0.0 && b > 0.0 && b <= a => {
                                if let Some(ws) = ws {
                                    ws.simulate(Analysis::Tran {
                                        tstop: a,
                                        tstep: b,
                                        power_on: *power_on,
                                    });
                                }
                                close = true;
                            }
                            (Ok(_), Ok(_)) => {
                                *error = Some("both must be positive, the resolution no larger than the stop time".into())
                            }
                            (Err(e), _) | (_, Err(e)) => *error = Some(e.to_string()),
                        }
                    }
                }
                Prompt::Ac { start, stop, per_decade, error } => {
                    ui.heading("AC Sweep");
                    ui.label(
                        "Small-signal gain and phase around the operating point, driven by \
                         each source's AC value (a sine source's ac parameter).",
                    );
                    egui::Grid::new("ac").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("From");
                        let r = ui.add(egui::TextEdit::singleline(start).hint_text("1, 10…"));
                        if ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                        ui.label("Hz");
                        ui.end_row();
                        ui.label("To");
                        ui.add(egui::TextEdit::singleline(stop).hint_text("1G"));
                        ui.label("Hz");
                        ui.end_row();
                        ui.label("Points per decade");
                        ui.add(egui::TextEdit::singleline(per_decade));
                        ui.end_row();
                    });
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    ui.add_space(4.0);
                    let run = ui.button("Run").clicked();
                    if enter || run {
                        let parse = |t: &str| circuiter_model::units::parse_quantity(t, Some("Hz"));
                        match (parse(start), parse(stop), per_decade.trim().parse::<usize>()) {
                            (Ok(a), Ok(b), Ok(n)) if a > 0.0 && b > a && (1..=1000).contains(&n) => {
                                if let Some(ws) = ws {
                                    ws.simulate(Analysis::Ac { fstart: a, fstop: b, per_decade: n });
                                }
                                close = true;
                            }
                            (Err(e), _, _) | (_, Err(e), _) => *error = Some(e.to_string()),
                            _ => {
                                *error = Some(
                                    "the start must be positive and below the stop, with 1 to 1000 points per decade"
                                        .into(),
                                )
                            }
                        }
                    }
                }
                Prompt::Import { file, defs, selected, outcome } => {
                    ui.heading(format!("Import from {file}"));
                    match outcome {
                        None => {
                            ui.label("Each becomes a part in this project, with a symbol.");
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                for (d, on) in defs.iter().zip(selected.iter_mut()) {
                                    ui.horizontal(|ui| {
                                        ui.checkbox(on, RichText::new(crate::import::definition_name(d)).monospace());
                                        ui.label(RichText::new(crate::import::describe(d)).weak());
                                    });
                                }
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let count = selected.iter().filter(|s| **s).count();
                                let go = ui.add_enabled(count > 0, egui::Button::new(format!("Import {count}"))).clicked();
                                if go || (enter && count > 0) {
                                    if let Some(ws) = ws {
                                        let chosen: Vec<_> = defs
                                            .iter()
                                            .zip(selected.iter())
                                            .filter(|(_, s)| **s)
                                            .map(|(d, _)| d.clone())
                                            .collect();
                                        *outcome = Some(ws.import_models(&chosen));
                                    }
                                }
                                if ui.button("Select none").clicked() {
                                    selected.iter_mut().for_each(|s| *s = false);
                                }
                            });
                        }
                        Some(results) => {
                            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                for r in results.iter() {
                                    match r {
                                        Ok((name, None)) => {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.colored_label(OK, "✔");
                                                ui.label(RichText::new(name).monospace());
                                            });
                                        }
                                        Ok((name, Some(w))) => {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.colored_label(WARNING, "⚠");
                                                ui.label(RichText::new(name).monospace());
                                                ui.label(RichText::new(format!("imported, but can't be simulated yet: {w}")).weak());
                                            });
                                        }
                                        Err(e) => {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.colored_label(ERROR, "✖");
                                                ui.label(e.as_str());
                                            });
                                        }
                                    }
                                }
                            });
                            ui.add_space(6.0);
                            if ui.button("Done").clicked() || enter {
                                close = true;
                            }
                        }
                    }
                }
                Prompt::SymbolText { text } => {
                    ui.heading("Text");
                    ui.label("${name} shows the reference (U1), ${param.X} a parameter's value.");
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text("${name}, ${param.W}, or any text")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    if enter && !text.trim().is_empty() {
                        if let Some(ws) = ws {
                            ws.tool = Tool::Text { text: text.trim().to_owned() };
                        }
                        close = true;
                    }
                }
                Prompt::Note { text } => {
                    ui.heading("Note");
                    ui.label("Text on the sheet. Ctrl+Enter places it; Enter starts a new line.");
                    let r = ui.add(
                        egui::TextEdit::multiline(text)
                            .hint_text("What this part of the circuit does")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    let place = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                    if (place || ui.button("Place Note").clicked()) && !text.trim().is_empty() {
                        if let Some(ws) = ws {
                            ws.tool = Tool::Note { text: text.trim().to_owned() };
                        }
                        close = true;
                    }
                }
                Prompt::AddParam { name, kind, unit, options, default, error } => {
                    ui.heading("Add Parameter");
                    ui.label("A value set on each placed instance, like a resistance or width.");
                    egui::Grid::new("add-param").num_columns(2).spacing([10.0, 6.0]).show(
                        ui,
                        |ui| {
                            ui.label("Name");
                            let r =
                                ui.add(egui::TextEdit::singleline(name).hint_text("R, W, gain…"));
                            if ui.memory(|m| m.focused().is_none()) {
                                r.request_focus();
                            }
                            ui.end_row();
                            ui.label("Kind");
                            egui::ComboBox::from_id_salt("param-kind")
                                .selected_text(kind.label())
                                .show_ui(ui, |ui| {
                                    for k in ParamChoice::ALL {
                                        ui.selectable_value(kind, k, k.label());
                                    }
                                });
                            ui.end_row();
                            if *kind == ParamChoice::Real {
                                ui.label("Unit");
                                ui.add(egui::TextEdit::singleline(unit).hint_text("Ω, F, V, m…"));
                                ui.end_row();
                            }
                            if *kind == ParamChoice::Choice {
                                ui.label("Options");
                                ui.add(
                                    egui::TextEdit::singleline(options)
                                        .hint_text("fast, typical, slow"),
                                );
                                ui.end_row();
                            }
                            ui.label("Default");
                            ui.add(
                                egui::TextEdit::singleline(default).hint_text("empty = required"),
                            );
                            ui.end_row();
                        },
                    );
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    ui.add_space(4.0);
                    let add = ui.button("Add").clicked();
                    if (enter || add) && !name.trim().is_empty() {
                        let param = ParamDef {
                            name: name.trim().to_owned(),
                            kind: match kind {
                                ParamChoice::Real => {
                                    ParamKind::Real { unit: unit.trim().to_owned() }
                                }
                                ParamChoice::Integer => ParamKind::Integer,
                                ParamChoice::Bool => ParamKind::Bool,
                                ParamChoice::Text => ParamKind::Text,
                                ParamChoice::Choice => ParamKind::Choice(
                                    options
                                        .split(',')
                                        .map(|o| o.trim().to_owned())
                                        .filter(|o| !o.is_empty())
                                        .collect(),
                                ),
                            },
                            default: Some(default.trim().to_owned()).filter(|d| !d.is_empty()),
                            description: String::new(),
                        };
                        if let Some(ws) = ws {
                            match ws.edit(|_, _, d| interface::add_param(d, param)) {
                                Ok(()) => close = true,
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                }
                Prompt::NewCell { name, folder, error, part3d } => {
                    if *part3d {
                        ui.heading("New 3D Part");
                        ui.label(
                            "A part to model in 3D, as in a CAD program: an enclosure, a \
                             bracket, a knob, or a component's body. It opens in the 3D view.",
                        );
                    } else {
                        ui.heading("New Cell");
                        ui.label(
                            "An empty schematic in this project, ready to become a reusable part.",
                        );
                    }
                    let r =
                        ui.add(egui::TextEdit::singleline(name).hint_text("nand2, dff, opamp…"));
                    if !ui.memory(|m| m.focused().is_some()) {
                        r.request_focus();
                    }
                    ui.label(RichText::new("Folder").small());
                    let folders = ws
                        .as_ref()
                        .and_then(|ws| ws.design.library(ws.project.name()))
                        .map(|l| l.all_folders())
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(folder)
                                .hint_text("top level, or gates, analog/filters…")
                                .desired_width(300.0),
                        );
                        if let Some(f) = folder_menu(ui, &folders) {
                            *folder = f;
                        }
                    });
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter {
                        if let Some(ws) = ws {
                            match ws.new_cell(name.trim(), folder) {
                                Ok(()) if *part3d => {
                                    // Start from a box to shape, in the 3D view.
                                    let _ = ws.edit(|_, _, d| {
                                        d.model3d = Some(body_editor::starting_body());
                                        Ok(())
                                    });
                                    open_3d = true;
                                    close = true;
                                }
                                Ok(()) => close = true,
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                }
                Prompt::Folder { lib, parent, renaming, name, error } => {
                    ui.heading(if renaming.is_some() { "Rename Folder" } else { "New Folder" });
                    let place = if parent.is_empty() {
                        format!("In {lib}")
                    } else {
                        format!("In {lib} › {parent}/")
                    };
                    ui.label(RichText::new(place).weak());
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("gates, cpu, analog/filters…")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    ui.label(
                        RichText::new("Folders can hold folders: `a/b` makes both.").weak().small(),
                    );
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter {
                        if let Some(ws) = ws {
                            let path = if parent.is_empty() {
                                name.trim().to_owned()
                            } else {
                                format!("{parent}/{}", name.trim())
                            };
                            let done = match renaming {
                                Some(old) => ws.rename_folder(lib, old, &path),
                                None => ws.new_folder(lib, &path).map(drop),
                            };
                            match done {
                                Ok(()) => close = true,
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                }
                Prompt::MoveCell { id, folder, error } => {
                    ui.heading(format!("Move {}", id.cell));
                    ui.label(RichText::new(format!("To a folder in {}", id.library)).weak());
                    let folders = ws
                        .as_ref()
                        .and_then(|ws| ws.design.library(&id.library))
                        .map(|l| l.all_folders())
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(folder)
                                .hint_text("top level")
                                .desired_width(300.0),
                        );
                        if !ui.memory(|m| m.focused().is_some()) {
                            r.request_focus();
                        }
                        if let Some(f) = folder_menu(ui, &folders) {
                            *folder = f;
                        }
                    });
                    ui.label(
                        RichText::new("A folder that doesn't exist yet is made.").weak().small(),
                    );
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter {
                        if let Some(ws) = ws {
                            match ws.move_cell(id, folder) {
                                Ok(()) => close = true,
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                }
                Prompt::MakeComponent { name, folder, error } => {
                    ui.heading("Make Component");
                    ui.label(
                        "The selected parts and the wires between them become a new part. \
                         Each connection to the rest of the sheet becomes a port; GND and \
                         VDD stay connected by name. This sheet is left as it is.",
                    );
                    ui.add_space(4.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("Name: divider, nand2, amp…")
                            .desired_width(f32::INFINITY),
                    );
                    if !ui.memory(|m| m.focused().is_some()) {
                        r.request_focus();
                    }
                    ui.label(RichText::new("Folder").small());
                    let folders = ws
                        .as_ref()
                        .and_then(|ws| ws.design.library(ws.project.name()))
                        .map(|l| l.all_folders())
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(folder)
                                .hint_text("top level")
                                .desired_width(300.0),
                        );
                        if let Some(f) = folder_menu(ui, &folders) {
                            *folder = f;
                        }
                    });
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter {
                        if let Some(ws) = ws {
                            match ws.component_from_selection(name.trim(), folder) {
                                Ok(_) => close = true,
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                }
                Prompt::NewProgram { name, error } => {
                    ui.heading("New Program");
                    ui.label("A file in the project's programs folder. The ending picks the language.");
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("blink.s, sort.c, fib.rs, alu.sv…")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    if let Some(e) = error {
                        ui.colored_label(ERROR, e.as_str());
                    }
                    if enter {
                        program = Some(name.clone());
                    }
                }
                Prompt::Shortcuts => {
                    ui.heading("Keyboard Shortcuts");
                    ui.add_space(6.0);
                    egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                        for (key, what) in SHORTCUTS {
                            ui.label(RichText::new(*key).monospace().strong());
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    if ui.button("Close").clicked() || enter {
                        close = true;
                    }
                }
                Prompt::Discard { then: pending } => {
                    ui.heading("Unsaved Changes");
                    ui.label("Save your changes before continuing?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_first = true;
                            then = Some(pending.clone());
                        }
                        if ui.button("Discard").clicked() {
                            then = Some(pending.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });

        if close || then.is_some() {
            self.prompt = None;
        }
        if open_3d {
            self.open_body_3d();
        }
        if let Some(e) = failed {
            self.error = Some(e);
        }
        if let Some(name) = program {
            match self.new_program(&name) {
                Ok(()) => self.prompt = None,
                Err(e) => {
                    if let Some(Prompt::NewProgram { error, .. }) = &mut self.prompt {
                        *error = Some(e);
                    }
                }
            }
        }
        if save_first {
            self.save();
            if self.dirty() {
                return; // Saving failed; the error is shown.
            }
        }
        if let Some(action) = then {
            if let Some(ws) = &mut self.workspace {
                ws.dirty = false; // Discarded (or just saved).
            }
            self.run(ctx, action);
        }
    }
}

struct Keys {
    hand: bool,
    open: bool,
    save: bool,
    new_cell: bool,
    undo: bool,
    redo: bool,
    reload: bool,
    op: bool,
    tran: bool,
    run: bool,
    fit: bool,
    up: bool,
    back: bool,
    forward: bool,
    escape: bool,
    delete: bool,
    rotate: bool,
    mirror: bool,
    wire: bool,
    label: bool,
    port: bool,
    add: bool,
    search: bool,
    flip: bool,
    view: bool,
    ground: bool,
    duplicate: bool,
    vdd: bool,
    rect: bool,
    circle: bool,
    text: bool,
    no_connect: bool,
}

fn ac_prompt() -> Prompt {
    Prompt::Ac { start: "1".into(), stop: "1G".into(), per_decade: "20".into(), error: None }
}

fn new_port_prompt(place: bool) -> Prompt {
    Prompt::Port { name: String::new(), kind: PortKind::Input, place, error: None }
}

/// What to do on an empty sheet: how a component is built from other parts.
fn empty_sheet_guide(ui: &egui::Ui, sheet: egui::Rect, theme: &Theme) {
    let p = style::palette_for(theme.dark);
    let steps = [
        (style::icon::HAND, "Drag parts here from Components, or the column beside the sheet"),
        (style::icon::LINE_SEGMENTS, "Wire them: drag from one pin's end to another"),
        (style::icon::PLUG, "Add a port (P) for each connection the component should have"),
        (style::icon::PUZZLE_PIECE, "It's now a part: place it on other sheets from Components"),
    ];
    let painter = ui.painter_at(sheet);
    let top = sheet.center() - egui::vec2(220.0, 90.0);
    painter.text(
        top,
        egui::Align2::LEFT_TOP,
        "Build a component",
        egui::FontId::proportional(20.0),
        p.text,
    );
    for (k, (glyph, text)) in steps.iter().enumerate() {
        let y = top.y + 44.0 + k as f32 * 30.0;
        painter.text(
            egui::pos2(top.x + 4.0, y),
            egui::Align2::LEFT_TOP,
            *glyph,
            egui::FontId::proportional(17.0),
            p.accent,
        );
        painter.text(
            egui::pos2(top.x + 34.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            format!("{}. {text}", k + 1),
            egui::FontId::proportional(14.0),
            p.weak,
        );
    }
    painter.text(
        egui::pos2(top.x, top.y + 44.0 + 4.0 * 30.0 + 10.0),
        egui::Align2::LEFT_TOP,
        "Or build it on a sheet, select it, and use Make Component from Selection.",
        egui::FontId::proportional(13.0),
        p.weak.gamma_multiply(0.8),
    );
}

/// The Make Component dialog, starting in the folder of the sheet the parts are on.
fn make_component_prompt(ws: &Workspace) -> Prompt {
    let Prompt::NewCell { folder, .. } = new_cell_prompt(ws, false) else { unreachable!() };
    Prompt::MakeComponent { name: String::new(), folder, error: None }
}

/// The New Cell (or New 3D Part) dialog, starting in the folder of the project cell
/// being edited.
fn new_cell_prompt(ws: &Workspace, part3d: bool) -> Prompt {
    let folder = ws
        .current_def()
        .filter(|_| ws.current().cell.library == ws.project.name())
        .map(|d| d.folder.clone())
        .unwrap_or_default();
    Prompt::NewCell { name: String::new(), folder, error: None, part3d }
}

/// A menu of a library's folders; returns the one picked (empty for the top level).
fn folder_menu(ui: &mut egui::Ui, folders: &[String]) -> Option<String> {
    let mut picked = None;
    ui.menu_button("…", |ui| {
        for f in std::iter::once(String::new()).chain(folders.iter().cloned()) {
            let label = if f.is_empty() { "(top level)".to_owned() } else { format!("{f}/") };
            if ui.button(label).clicked() {
                picked = Some(f);
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Existing folders");
    picked
}

/// A section heading in the inspector, with a rule under it.
fn section(ui: &mut egui::Ui, title: &str) {
    let p = style::palette(ui);
    ui.add_space(12.0);
    ui.label(RichText::new(title).strong().size(13.5).color(p.text));
    let rect = ui.available_rect_before_wrap();
    ui.painter().hline(rect.x_range(), rect.top() + 1.0, egui::Stroke::new(1.0, p.border));
    ui.add_space(6.0);
}

/// The top of the inspector for a part: its symbol, name, cell and description.
fn part_card(
    ui: &mut egui::Ui,
    design: &circuiter_model::Design,
    id: &CellId,
    title: impl FnOnce(&mut egui::Ui),
    description: &str,
) {
    let p = style::palette(ui);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 52.0), egui::Sense::hover());
        ui.painter().rect(
            rect,
            6.0,
            p.well,
            egui::Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        if let Some(scene) = circuiter_scene::Scene::symbol_sheet(design, id) {
            canvas::draw_icon(ui.painter(), rect.shrink(9.0), &scene, p.text);
        }
        ui.vertical(|ui| {
            title(ui);
            ui.label(RichText::new(id.to_string()).monospace().color(p.weak));
        });
    });
    if !description.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(description).color(p.weak));
    }
}

fn instance_panel(ui: &mut egui::Ui, ws: &mut Workspace, index: usize, field: &mut Field) {
    let Some(schematic) = ws.schematic() else { return };
    let Some(inst) = schematic.instances.get(index).cloned() else { return };
    let editable = ws.editable();
    let lib = ws.current().cell.library.clone();
    let Some((id, def)) = ws.design.resolve(&inst.cell, &lib).map(|(id, d)| (id, d.clone())) else {
        ui.colored_label(ERROR, format!("unknown cell {}", inst.cell));
        return;
    };
    let key_base = format!("{}/{index}", ws.current().cell);

    part_card(
        ui,
        &ws.design,
        &id,
        |ui| {
            ui.label(RichText::new(&inst.name).size(20.0).strong());
        },
        &def.cell.description,
    );

    // Parts operated while running live, and meters' readings.
    bench::live_part_controls(ui, ws, index);
    if id == CellId::new(circuiter_model::STD_LIBRARY, "multimeter") {
        let path = ws.instance_path(index).unwrap_or_default();
        let reading = match &ws.live {
            Some(live) => live.shared.lock().ok().and_then(|s| s.readings.get(&path).copied()),
            None => ws.fresh_sim().and_then(|sim| {
                let meters = crate::live::Meter::all(&sim.netlist);
                meters.iter().find(|m| m.path == path).and_then(|m| sim.meter(m))
            }),
        };
        bench::live_meter_display(ui, reading);
    }

    section(ui, "General");
    style::properties(ui, "general", |ui| {
        style::name(ui, "Reference");
        if editable {
            if let Some(name) = field.edit(ui, &format!("{key_base}/name"), &inst.name, 150.0) {
                match ws.edit(|_, _, d| crate::edit::rename(d, index, name.trim())) {
                    Ok(()) => field.error = None,
                    Err(e) => field.error = Some((format!("{key_base}/name"), e)),
                }
            }
        } else {
            style::value(ui, &inst.name, 150.0);
        }
        ui.end_row();
        style::name(ui, "Component");
        style::value(ui, &id.cell, 150.0);
        ui.end_row();
        let at = inst.placement.position;
        style::name(ui, "Position");
        style::value(ui, &format!("{:.2}, {:.2} mm", at.x.mm(), at.y.mm()), 150.0);
        ui.end_row();
        style::name(ui, "Rotation");
        let degrees = match inst.placement.rotation {
            circuiter_model::Rotation::R0 => 0,
            circuiter_model::Rotation::R90 => 90,
            circuiter_model::Rotation::R180 => 180,
            circuiter_model::Rotation::R270 => 270,
        };
        let mirror = if inst.placement.mirror { ", mirrored" } else { "" };
        style::value(ui, &format!("{degrees}°{mirror}"), 150.0);
        ui.end_row();
    });

    // KiCad's fields: the value and footprint the board and the BOM use, and the
    // part's numbers for buying it.
    if def.footprint.is_some() || !inst.fields.is_empty() {
        section(ui, "Fields");
        style::properties(ui, "fields", |ui| {
            style::name(ui, "Value");
            style::value(ui, &circuiter_pcb::layout::value_of(&def, &inst.params), 150.0);
            ui.end_row();
            if let Some(view) = &def.footprint {
                let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
                let param = |n: &str| {
                    inst.params.get(n).cloned().or_else(|| def.cell.param(n)?.default.clone())
                };
                let name = circuiter_pcb::footprints::resolve(view, &ports, &param)
                    .map_or_else(|e| e, |f| f.name);
                style::name(ui, "Footprint");
                style::value(ui, &name, 150.0).on_hover_text(&name);
                ui.end_row();
            }
            const STANDARD: [&str; 3] = ["MPN", "Manufacturer", "Datasheet"];
            let mut names: Vec<String> = STANDARD.map(str::to_owned).to_vec();
            names.extend(
                inst.fields
                    .keys()
                    .filter(|k| !STANDARD.contains(&k.as_str()) && *k != "DNP")
                    .cloned(),
            );
            for name in names {
                style::name(ui, &name);
                let current = inst.fields.get(&name).cloned().unwrap_or_default();
                if editable {
                    let key = format!("{key_base}/field/{name}");
                    if let Some(v) = field.edit(ui, &key, &current, 150.0) {
                        let result = ws.edit(|_, _, d| {
                            let s = d.schematic.as_mut().ok_or("no schematic")?;
                            let i = s.instances.get_mut(index).ok_or("no part")?;
                            match v.trim() {
                                "" => drop(i.fields.remove(&name)),
                                v => drop(i.fields.insert(name.clone(), v.to_owned())),
                            }
                            Ok(())
                        });
                        if let Err(e) = result {
                            field.error = Some((key, e));
                        }
                    }
                } else {
                    style::value(ui, &current, 150.0);
                }
                ui.end_row();
            }
            style::name(ui, "Do not populate");
            let mut dnp = inst.dnp();
            if ui
                .add_enabled(editable, egui::Checkbox::without_text(&mut dnp))
                .on_hover_text("Left off the assembled board: listed as DNP in the bill of materials, left out of the placement file")
                .changed()
            {
                let _ = ws.edit(|_, _, d| {
                    let s = d.schematic.as_mut().ok_or("no schematic")?;
                    let i = s.instances.get_mut(index).ok_or("no part")?;
                    if dnp {
                        i.fields.insert("DNP".into(), "true".into());
                    } else {
                        i.fields.remove("DNP");
                    }
                    Ok(())
                });
            }
            ui.end_row();
        });
    }

    section(ui, "Model");
    let model = match ws.design.select_view(&def, inst.view) {
        Some(ViewKind::Schematic) => "Schematic (hierarchical)",
        Some(ViewKind::Spice) if def.digital.is_some() => "SPICE model, and a digital model",
        Some(ViewKind::Spice) => "SPICE model",
        Some(ViewKind::Digital) => "Digital model",
        None if def.digital.is_some() => "Digital model",
        None if def.cell.global_net.is_some() => "Global net",
        None => "No simulation view",
    };
    ui.label(model);
    // Parts with several models: which one simulations use for this part.
    let views: Vec<(Option<ViewKind>, &str)> = [
        (Some(ViewKind::Digital), "Digital model (fast)"),
        (Some(ViewKind::Spice), "Transistor level (SPICE)"),
        (Some(ViewKind::Schematic), "Its schematic (open it to see)"),
    ]
    .into_iter()
    .filter(|(v, _)| v.is_some_and(|v| def.has_view(v)))
    .collect();
    if views.len() > 1 && editable {
        let current = views.iter().find(|(v, _)| *v == inst.view).map_or("Automatic", |(_, l)| l);
        let mut chosen = None;
        ui.horizontal(|ui| {
            style::name(ui, "Simulate as");
            egui::ComboBox::from_id_salt(("simulate-as", index))
                .selected_text(current)
                .width(200.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(inst.view.is_none(), "Automatic")
                        .on_hover_text(
                            "Digital in digital and mixed designs; transistor level in analog ones",
                        )
                        .clicked()
                    {
                        chosen = Some(None);
                    }
                    for (v, label) in &views {
                        if ui.selectable_label(inst.view == *v, *label).clicked() {
                            chosen = Some(*v);
                        }
                    }
                });
        });
        if let Some(view) = chosen {
            let _ = ws.edit(|_, _, d| {
                if let Some(i) = d.schematic.as_mut().and_then(|s| s.instances.get_mut(index)) {
                    i.view = view;
                }
                Ok(())
            });
        }
    }
    if ws.scene.descendable.get(index) == Some(&true) && ui.button("Open schematic").clicked() {
        ws.descend(index);
        return;
    }

    if !def.cell.params.is_empty() {
        section(ui, "Parameters");
        let mut change: Option<(String, Option<String>)> = None;
        egui::Grid::new("params").num_columns(3).striped(true).spacing([10.0, 4.0]).show(
            ui,
            |ui| {
                for p in &def.cell.params {
                    let overridden = inst.params.contains_key(&p.name);
                    ui.label(RichText::new(&p.name).monospace()).on_hover_text(&p.description);
                    let raw = inst
                        .params
                        .get(&p.name)
                        .or(p.default.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    if editable {
                        let key = format!("{key_base}/param/{}", p.name);
                        if let Some(text) = field.edit(ui, &key, &raw, 110.0) {
                            let text = text.trim().to_owned();
                            if text.is_empty() {
                                change = Some((p.name.clone(), None));
                                field.error = None;
                            } else {
                                match p.kind.validate(&text) {
                                    Ok(()) => {
                                        change = Some((p.name.clone(), Some(text)));
                                        field.error = None;
                                    }
                                    Err(e) => field.error = Some((key, e)),
                                }
                            }
                        }
                    } else {
                        ui.label(RichText::new(&raw).monospace());
                    }
                    let shown = RichText::new(param_display(&inst, &def, &p.name)).monospace();
                    ui.horizontal(|ui| {
                        ui.label(if overridden { shown.strong() } else { shown.weak() });
                        if editable
                            && overridden
                            && ui.small_button("↺").on_hover_text("Reset to default").clicked()
                        {
                            change = Some((p.name.clone(), None));
                        }
                    });
                    ui.end_row();
                }
            },
        );
        if let Some((name, value)) = change {
            let _ = ws.edit(|_, _, d| {
                crate::edit::set_param(d, index, &name, value);
                Ok(())
            });
        }
    }

    if let Some(sim) = ws.fresh_sim() {
        let path = match ws.sheet_path() {
            p if p.is_empty() => inst.name.clone(),
            p => format!("{p}.{}", inst.name),
        };
        if let Some((currents, power)) = sim.element_op(&path) {
            section(ui, "Operating point");
            egui::Grid::new("op").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                for (port, i) in &currents {
                    ui.label(RichText::new(format!("I({port})")).monospace())
                        .on_hover_text("Current flowing into this terminal");
                    ui.label(RichText::new(show(*i, "A")).monospace());
                    ui.end_row();
                }
                ui.label(RichText::new("Power").monospace())
                    .on_hover_text("Absorbed; negative when the part delivers power");
                ui.label(RichText::new(show(power, "W")).monospace().color(ACCENT));
                ui.end_row();
            });
        } else if let Some(power) = sim.power_under(&path) {
            section(ui, "Operating point");
            ui.label(RichText::new(format!("Total power  {}", show(power, "W"))).monospace());
        }
        if let Some(a) = sim.assessments.iter().find(|a| a.path == path) {
            ui.add_space(4.0);
            let when = match a.time_to_failure {
                Some(0.0) => " at once".to_owned(),
                Some(t) => format!(" in about {}", show(t, "s")),
                None => String::new(),
            };
            ui.colored_label(WARNING, format!("⚠ Would fail{when}: {}", a.reason));
        }
    }
    if let Some(sim) = ws.fresh_sim() {
        let path = match ws.sheet_path() {
            p if p.is_empty() => inst.name.clone(),
            p => format!("{p}.{}", inst.name),
        };
        if let Some(part) = sim.monitor.as_ref().and_then(|m| m.part(&path)) {
            section(ui, "Heat");
            egui::Grid::new("heat").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("Temperature");
                ui.label(RichText::new(format!("{:.1} °C", part.temperature)).monospace());
                ui.end_row();
                ui.label("Peak");
                ui.label(RichText::new(format!("{:.1} °C", part.peak)).monospace());
                ui.end_row();
                if let Some(max) = part.max_temperature() {
                    ui.label("Destroyed at");
                    ui.label(RichText::new(format!("{max:.0} °C")).monospace());
                    ui.end_row();
                }
                ui.label(if part.failed { "Power when it failed" } else { "Power at the end" });
                ui.label(RichText::new(show(part.power, "W")).monospace());
                ui.end_row();
            });
        }
        if let Some(e) = sim.failure(&path) {
            ui.add_space(4.0);
            ui.colored_label(ERROR, format!("✖ {}", e.describe()));
        }
    }

    let conn = &ws.scene.connectivity;
    let pins: Vec<(usize, String)> = conn
        .pins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.instance == index)
        .map(|(i, p)| (i, p.port.clone()))
        .collect();
    if !pins.is_empty() {
        section(ui, "Connections");
        let mut go_to_net = None;
        egui::Grid::new("pins").num_columns(2).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for (pin, port) in &pins {
                ui.label(RichText::new(port).monospace());
                match conn.net_of(Item::Pin(*pin)) {
                    Some(_) if conn.dangling_pins.contains(pin) => {
                        ui.colored_label(ERROR, "not connected");
                    }
                    Some(n) => {
                        if ui.link(RichText::new(&conn.nets[n].name).monospace()).clicked() {
                            go_to_net = Some(n);
                        }
                    }
                    None => {
                        ui.label("—");
                    }
                }
                ui.end_row();
            }
        });
        if let Some(n) = go_to_net {
            ws.select_net(n);
        }
    }
}

/// The right-click menu of a net on the sheet.
fn net_menu(ui: &mut egui::Ui, ws: &mut Workspace, net: usize) {
    let Some(name) = ws.scene.connectivity.nets.get(net).map(|n| n.name.clone()) else { return };
    ui.label(RichText::new(&name).monospace().weak());
    ui.separator();
    match ws.plotted(net) {
        Some(plotted) => {
            if ui.button(if plotted { "Remove from Plot" } else { "Add to Plot" }).clicked() {
                ws.toggle_plot(net);
            }
        }
        None => {
            let why = if ws.sim.is_some() {
                "The results are out of date or didn't trace this net: run again (Ctrl+R)"
            } else {
                "Run the simulation first (Ctrl+R)"
            };
            ui.add_enabled(false, egui::Button::new("Add to Plot")).on_disabled_hover_text(why);
        }
    }
    if ui.button("Copy Net Name").clicked() {
        ui.ctx().copy_text(name);
    }
}

fn net_panel(ui: &mut egui::Ui, ws: &mut Workspace, net: usize) {
    if ws.schematic().is_none() {
        return;
    }
    let conn = &ws.scene.connectivity;
    let Some(n) = conn.nets.get(net) else { return };
    ui.label(RichText::new("Net").weak());
    ui.label(RichText::new(&n.name).size(22.0).strong().monospace());
    let source = match n.source {
        NetNameSource::Global => "Global net",
        NetNameSource::Port => "Named by a port of this cell",
        NetNameSource::Label => "Named by a net label",
        NetNameSource::Auto => "Unnamed (automatic name)",
    };
    ui.label(RichText::new(source).weak());
    if n.names.len() > 1 {
        ui.colored_label(WARNING, format!("⚠ conflicting names: {}", n.names.join(", ")));
    }
    let mut toggle = false;
    // Running live: the value now, and the live waveforms.
    let live_flat = ws.live_nets().get(net).copied().flatten();
    if let (Some(live), Some(flat)) = (&ws.live, live_flat) {
        section(ui, "Live");
        let (v, bits) = live
            .shared
            .lock()
            .map(|s| {
                (s.voltages.get(flat).copied().flatten(), s.logic.get(flat).copied().flatten())
            })
            .unwrap_or((None, None));
        let shown = v.map(|v| show(v, "V")).or(bits.map(|b| b.hex())).unwrap_or_else(|| "—".into());
        ui.label(RichText::new(shown).size(18.0).monospace().color(ACCENT));
        let watched = ws.live_watch.contains(&flat);
        if ui.button(if watched { "Remove from Plot" } else { "Add to Plot" }).clicked() {
            ws.toggle_live_watch(net);
        }
        return;
    }
    if let (Some(sim), Some(flat)) = (ws.fresh_sim(), ws.flat_net(net)) {
        section(ui, "Simulation");
        let flat_name = &sim.netlist.nets[flat].name;
        if *flat_name != n.name {
            ui.label(RichText::new(format!("Simulated as {flat_name}")).weak());
        }
        if let Some(v) = sim.voltage(flat) {
            ui.label(RichText::new(show(v, "V")).size(18.0).monospace().color(ACCENT));
        }
        if sim.waveform(flat).is_some() || sim.response(flat).is_some() || sim.logic(flat).is_some()
        {
            let plotted = sim.probes.contains(&flat);
            let label = if plotted { "Remove from Plot" } else { "Add to Plot" };
            toggle = ui.button(label).clicked();
        }
    }
    if toggle {
        ws.toggle_probe(net);
    }
    let Some(schematic) = ws.schematic() else { return };
    let conn = &ws.scene.connectivity;
    let Some(n) = conn.nets.get(net) else { return };
    section(ui, "Connected pins");
    let mut select = None;
    for item in &n.items {
        match *item {
            Item::Pin(p) => {
                let pin = &conn.pins[p];
                let inst = &schematic.instances[pin.instance];
                let text = RichText::new(format!("{}.{}", inst.name, pin.port)).monospace();
                if ui.link(text).on_hover_text(inst.cell.to_string()).clicked() {
                    select = Some(pin.instance);
                }
            }
            Item::Port(p) => {
                ui.label(RichText::new(format!("port {}", schematic.ports[p].port)).monospace());
            }
            _ => {}
        }
    }
    if let Some(i) = select {
        ws.select_only(Some(Owner::Instance(i)));
    }
}

/// Deferred edits, collected while drawing a panel and applied after it.
type CellChange = Box<dyn FnOnce(&mut circuiter_model::CellDef)>;
type PinChange = Box<dyn FnOnce(&mut circuiter_model::symbol::SymbolPin)>;
type GraphicChange = Box<dyn FnOnce(&mut Graphic)>;

/// The cell itself: its interface (editable unless it's a library part), symbol status,
/// contents and problems. Shown when nothing is selected.
fn cell_panel(
    ui: &mut egui::Ui,
    ws: &mut Workspace,
    field: &mut Field,
    prompt: &mut Option<Prompt>,
    open_3d: &mut bool,
    edit_footprint: &mut Option<circuiter_model::board::FootprintView>,
) {
    let Some(def) = ws.current_def().cloned() else { return };
    let id = ws.current().cell.clone();
    let editable = ws.editable();
    let key = |k: &str| format!("{id}/{k}");
    let commit =
        |ws: &mut Workspace,
         field: &mut Field,
         k: String,
         f: &dyn Fn(&mut circuiter_model::CellDef) -> Result<(), String>| {
            match ws.edit(|_, _, d| f(d)) {
                Ok(()) => field.error = None,
                Err(e) => field.error = Some((k, e)),
            }
        };

    ui.label(RichText::new(&def.name).size(22.0).strong().monospace());
    let place =
        if def.folder.is_empty() { id.to_string() } else { format!("{id}  ·  {}/", def.folder) };
    ui.label(RichText::new(place).monospace().weak());
    if editable {
        egui::Grid::new("cell-meta").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label("Folder")
                .on_hover_text("Where the cell is listed and stored: gates, analog/filters…");
            let folders =
                ws.design.library(&id.library).map(|l| l.all_folders()).unwrap_or_default();
            let chosen = ui
                .horizontal(|ui| {
                    let typed =
                        field.edit_hinted(ui, &key("folder"), &def.folder, 160.0, "top level");
                    let picked = folder_menu(ui, &folders);
                    typed.or(picked)
                })
                .inner;
            if let Some(folder) = chosen {
                match ws.move_cell(&id, &folder) {
                    Ok(()) => field.error = None,
                    Err(e) => field.error = Some((key("folder"), e)),
                }
            }
            ui.end_row();
            ui.label("Description");
            if let Some(text) = field.edit(ui, &key("description"), &def.cell.description, 190.0) {
                commit(ws, field, key("description"), &|d| {
                    interface::set_description(d, &text);
                    Ok(())
                });
            }
            ui.end_row();
            ui.label("Prefix").on_hover_text("Reference designators: U1, U2…");
            if let Some(text) = field.edit(ui, &key("prefix"), &def.cell.prefix, 60.0) {
                commit(ws, field, key("prefix"), &|d| interface::set_prefix(d, &text));
            }
            ui.end_row();
        });
    } else if !def.cell.description.is_empty() {
        ui.label(&def.cell.description);
    }

    // Ports.
    if editable || !def.cell.ports.is_empty() {
        section(ui, "Ports");
    }
    let n_ports = def.cell.ports.len();
    let mut action: Option<CellChange> = None;
    egui::Grid::new("ports").num_columns(3).striped(true).spacing([8.0, 4.0]).show(ui, |ui| {
        for (i, p) in def.cell.ports.iter().enumerate() {
            if !editable {
                ui.label(RichText::new(&p.name).monospace());
                ui.label(RichText::new(kind_label(p.kind)).weak());
                ui.end_row();
                continue;
            }
            let k = key(&format!("port/{}", p.name));
            if let Some(new) = field.edit(ui, &k, &p.name, 80.0) {
                let old = p.name.clone();
                commit(ws, field, k, &|d| interface::rename_port(d, &old, new.trim()));
            }
            let mut kind = p.kind;
            egui::ComboBox::from_id_salt(("port-kind", i))
                .selected_text(kind_label(kind))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for k in PORT_KINDS {
                        ui.selectable_value(&mut kind, k, kind_label(k));
                    }
                });
            if kind != p.kind {
                let name = p.name.clone();
                action = Some(Box::new(move |d| interface::set_port_kind(d, &name, kind)));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(i > 0, egui::Button::new("⏶").small())
                    .on_hover_text("Move up")
                    .clicked()
                {
                    action = Some(Box::new(move |d| interface::move_port(d, i, true)));
                }
                if ui
                    .add_enabled(i + 1 < n_ports, egui::Button::new("⏷").small())
                    .on_hover_text("Move down")
                    .clicked()
                {
                    action = Some(Box::new(move |d| interface::move_port(d, i, false)));
                }
                if ui.small_button("✖").on_hover_text("Remove port").clicked() {
                    let name = p.name.clone();
                    action = Some(Box::new(move |d| interface::remove_port(d, &name)));
                }
            });
            ui.end_row();
        }
    });
    if editable && ui.small_button("+ Add Port").clicked() {
        *prompt = Some(new_port_prompt(ws.mode == Mode::Schematic && ws.has_schematic()));
    }

    // Parameters.
    if editable || !def.cell.params.is_empty() {
        section(ui, "Parameters");
    }
    egui::Grid::new("cell-params").num_columns(3).striped(true).spacing([8.0, 4.0]).show(
        ui,
        |ui| {
            for p in &def.cell.params {
                ui.label(RichText::new(&p.name).monospace())
                    .on_hover_text(param_kind_label(&p.kind));
                let current = p.default.clone().unwrap_or_default();
                if editable {
                    let k = key(&format!("param/{}", p.name));
                    if let Some(text) = field.edit(ui, &k, &current, 90.0) {
                        let name = p.name.clone();
                        let value = Some(text.trim().to_owned()).filter(|t| !t.is_empty());
                        commit(ws, field, k, &|d| {
                            interface::set_param_default(d, &name, value.clone())
                        });
                    }
                    ui.horizontal(|ui| {
                        if p.default.is_none() {
                            ui.label(RichText::new("required").weak().small());
                        }
                        if ui.small_button("✖").on_hover_text("Remove parameter").clicked() {
                            let name = p.name.clone();
                            action = Some(Box::new(move |d| interface::remove_param(d, &name)));
                        }
                    });
                } else {
                    ui.label(RichText::new(p.default.as_deref().unwrap_or("required")).monospace());
                    ui.label(RichText::new(param_kind_label(&p.kind)).weak().small());
                }
                ui.end_row();
            }
        },
    );
    if editable && ui.small_button("+ Add Parameter").clicked() {
        *prompt = Some(Prompt::AddParam {
            name: String::new(),
            kind: ParamChoice::Real,
            unit: String::new(),
            options: String::new(),
            default: String::new(),
            error: None,
        });
    }
    if let Some(f) = action {
        let _ = ws.edit(|_, _, d| {
            f(d);
            Ok(())
        });
    }

    if ws.mode == Mode::Schematic && def.schematic.is_some() {
        frame_section(ui, ws, field);
    }

    // Symbol.
    if def.cell.global_net.is_none() {
        section(ui, "Symbol");
        let unpinned = symbol_edit::unpinned_ports(&def);
        let status = match &def.symbol {
            None if def.cell.ports.is_empty() => {
                "None yet. Add a port and one is generated, so the cell can be placed."
            }
            None => "None, so the cell can't be placed on other sheets.",
            Some(_) if builder::is_generated(&def) => "Generated from the ports; follows them.",
            Some(_) => "Custom drawing.",
        };
        ui.label(RichText::new(status).weak());
        if editable && !unpinned.is_empty() && def.symbol.is_some() {
            ui.colored_label(WARNING, format!("No pin yet: {}", unpinned.join(", ")));
        }
        ui.horizontal(|ui| {
            if ws.mode == Mode::Schematic && ui.button("Edit Symbol").on_hover_text("V").clicked() {
                ws.set_mode(Mode::Symbol);
            }
            if editable
                && !unpinned.is_empty()
                && def.symbol.is_some()
                && ui.button("Place Pins").clicked()
            {
                if let Err(e) = ws.start_pin(None) {
                    field.error = Some((key("symbol"), e));
                }
            }
            let generated = builder::is_generated(&def);
            if editable && !def.cell.ports.is_empty() && !generated {
                let label = if def.symbol.is_some() { "Regenerate" } else { "Generate" };
                let r = ui.button(label).on_hover_text(
                    "Replace the symbol with a box generated from the ports (undo with Ctrl+Z)",
                );
                if r.clicked() {
                    let _ = ws.edit(|_, _, d| {
                        symbol_edit::generate(d);
                        Ok(())
                    });
                }
            }
        });
        // Sections of a generated symbol, as a big part's banks are drawn.
        if editable && builder::is_generated(&def) && def.cell.ports.len() > 1 {
            egui::CollapsingHeader::new("Sections").id_salt(key("sections")).show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Ports with the same section are drawn together, under its title \
                         and ruled off from the rest, as a big part's banks are.",
                    )
                    .weak(),
                );
                let mut set: Option<(String, String)> = None;
                egui::Grid::new("sections").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for p in def.cell.ports.iter().filter(|p| p.global.is_none()) {
                        ui.label(RichText::new(&p.name).monospace());
                        let k = key(&format!("port/{}/group", p.name));
                        if let Some(group) = field.edit_hinted(ui, &k, &p.group, 120.0, "none") {
                            set = Some((p.name.clone(), group));
                        }
                        ui.end_row();
                    }
                });
                if let Some((name, group)) = set {
                    let _ = ws.edit(|_, _, d| {
                        interface::set_port_group(d, &name, &group);
                        Ok(())
                    });
                }
            });
        }
    }

    // Its physical body.
    section(ui, "3D body");
    let status = match &def.model3d {
        Some(m) if !m.features.is_empty() => format!("Modelled: {} feature(s).", m.features.len()),
        Some(m) if !m.package.is_empty() => format!("Made by the `{}` package.", m.package),
        _ => "None yet.".to_owned(),
    };
    ui.label(RichText::new(status).weak());
    let label = if def.model3d.is_some() { "Open in 3D" } else { "Model a 3D Body" };
    if ui
        .button(format!("{}  {label}", style::icon::CUBE))
        .on_hover_text("Shape its body in the 3D view, as in a CAD program")
        .clicked()
    {
        *open_3d = true;
    }

    // Its land on the board, for cells that are parts rather than circuits of parts.
    section(ui, "Footprint");
    if let Some(view) = footprint::section(ui, ws) {
        *edit_footprint = Some(view);
    }

    if let Some(schematic) = ws.schematic() {
        let conn = &ws.scene.connectivity;
        section(ui, "Contents");
        ui.label(format!(
            "{} instances · {} nets · {} junctions",
            schematic.instances.len(),
            conn.nets.len(),
            conn.junctions.len()
        ));
        if schematic.instances.is_empty() && ws.editable() {
            ui.add_space(6.0);
            ui.label(RichText::new("Press A to add a part, W to draw wires.").weak());
        }
    }

    if ws.mode == Mode::Schematic && ws.has_schematic() {
        section(ui, "Simulate");
        let running = ws.sim.as_ref().is_some_and(|s| s.job.is_some());
        let ran = ws.sim.as_ref().is_some_and(|s| s.results.is_some());
        if !running && ui.button(RichText::new("▶ Run").strong().color(ACCENT)).clicked() {
            let a = ws.run_analysis();
            ws.simulate(a);
        }
        ui.label(format!(
            "Run (Ctrl+R) switches {} on and watches it {}: the waveforms appear \
             below the sheet, each wire shows its voltage, and parts that overheat are marked.",
            ws.sim_top(),
            ws.run_span()
        ));
        if !ran {
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Analyses has more: Operating Point (steady-state voltages, currents and \
                     power), Transient (a duration you choose) and AC Sweep (gain and phase \
                     over frequency).",
                )
                .weak(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "A circuit needs a source (std:vdc, std:vsine…) and a ground (std:gnd). \
                     Logic gates draw their power from VDD: connect a source to a VDD symbol \
                     (std:vdd).",
                )
                .weak(),
            );
        } else {
            ui.label(
                RichText::new("Select a wire to see its voltage; Add to Plot graphs it.").weak(),
            );
        }
    }

    section(ui, "Problems");
    let problems = ws.problems();
    if problems.is_empty() {
        ui.label(RichText::new("✔ None").color(OK));
    }
    for (severity, message) in problems {
        let (icon, c) = match severity {
            Severity::Error => ("✖", ERROR),
            Severity::Warning => ("⚠", WARNING),
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c, icon);
            ui.label(message);
        });
    }
}

/// A symbol pin: which port it is, the side it's on, its length and name display.
fn pin_panel(ui: &mut egui::Ui, ws: &mut Workspace, index: usize, field: &mut Field) {
    let Some(pin) = ws.symbol().and_then(|s| s.pins.get(index)).cloned() else { return };
    let kind = ws.current_def().and_then(|d| d.cell.port(&pin.port)).map(|p| p.kind);
    let editable = ws.editable();
    ui.label(RichText::new("Pin").weak());
    ui.label(RichText::new(&pin.port).size(22.0).strong().monospace());
    if let Some(kind) = kind {
        ui.label(RichText::new(kind_label(kind)).weak());
    }
    section(ui, "Placement");
    let key = format!("{}/pin/{index}", ws.current().cell);
    let mut change: Option<PinChange> = None;
    egui::Grid::new("pin").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label("Side");
        ui.add_enabled_ui(editable, |ui| {
            let mut direction = pin.direction;
            egui::ComboBox::from_id_salt("pin-side").selected_text(side_label(direction)).show_ui(
                ui,
                |ui| {
                    for d in [
                        PinDirection::Right,
                        PinDirection::Left,
                        PinDirection::Down,
                        PinDirection::Up,
                    ] {
                        ui.selectable_value(&mut direction, d, side_label(d));
                    }
                },
            );
            if direction != pin.direction {
                change = Some(Box::new(move |p| p.direction = direction));
            }
        });
        ui.end_row();
        ui.label("Connects at");
        ui.label(RichText::new(format!("{} mm", pin.at)).monospace());
        ui.end_row();
        ui.label("Length");
        if editable {
            let k = format!("{key}/length");
            if let Some(text) = field.edit(ui, &k, &format!("{}", pin.length.mm()), 70.0) {
                match parse_length(&text) {
                    Ok(len) => {
                        field.error = None;
                        change = Some(Box::new(move |p| p.length = len));
                    }
                    Err(e) => field.error = Some((k, e)),
                }
            }
        } else {
            ui.label(format!("{} mm", pin.length.mm()));
        }
        ui.end_row();
        ui.label("");
        let mut show = pin.show_name;
        if ui.add_enabled(editable, egui::Checkbox::new(&mut show, "Show name")).changed() {
            change = Some(Box::new(move |p| p.show_name = show));
        }
        ui.end_row();
    });
    if let Some(f) = change {
        let _ = ws.edit(|_, _, d| {
            let pin = d.symbol.as_mut().and_then(|s| s.pins.get_mut(index)).ok_or("no pin")?;
            f(pin);
            Ok(())
        });
    }
}

/// A symbol drawing primitive: fill, size and text.
/// A note on the sheet: a group box's title, or a note's text.
fn note_panel(ui: &mut egui::Ui, ws: &mut Workspace, index: usize, field: &mut Field) {
    use circuiter_model::schematic::Note;
    let Some(note) = ws.schematic().and_then(|s| s.notes.get(index)).cloned() else { return };
    let editable = ws.editable();
    let key = format!("{}/note/{index}", ws.current().cell);
    ui.label(RichText::new("Note").weak());
    let set = |ws: &mut Workspace, note: Note| {
        let _ = ws.edit(|_, _, d| {
            if let Some(n) = d.schematic.as_mut().and_then(|s| s.notes.get_mut(index)) {
                *n = note;
            }
            Ok(())
        });
    };
    match note {
        Note::Box { a, b, title } => {
            ui.label(RichText::new("Group box").size(22.0).strong());
            section(ui, "Properties");
            egui::Grid::new("note").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label("Title");
                if !editable {
                    ui.label(&title);
                } else if let Some(t) =
                    field.edit_hinted(ui, &format!("{key}/title"), &title, 180.0, "Power supply")
                {
                    set(ws, Note::Box { a, b, title: t.trim().to_owned() });
                }
                ui.end_row();
                ui.label("Size");
                ui.label(format!(
                    "{:.2} × {:.2} mm",
                    (b.x - a.x).mm().abs(),
                    (b.y - a.y).mm().abs()
                ));
                ui.end_row();
            });
        }
        Note::Text { at, text } => {
            ui.label(RichText::new("Text").size(22.0).strong());
            section(ui, "Text");
            let id = egui::Id::new(&key);
            let mut buffer =
                ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| text.clone());
            let r = ui.add_enabled(
                editable,
                egui::TextEdit::multiline(&mut buffer).desired_rows(4).desired_width(f32::INFINITY),
            );
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(id, buffer.clone()));
            }
            if r.lost_focus() {
                ui.data_mut(|d| d.remove::<String>(id));
                if buffer.trim() != text && !buffer.trim().is_empty() {
                    set(ws, Note::Text { at, text: buffer.trim().to_owned() });
                }
            }
        }
    }
    if editable {
        ui.add_space(8.0);
        if ui.button(format!("{}  Delete", style::icon::TRASH)).clicked() {
            ws.delete_selection();
        }
    }
}

/// The sheet's frame and title block: whether it has one, the paper and the fields.
fn frame_section(ui: &mut egui::Ui, ws: &mut Workspace, field: &mut Field) {
    use circuiter_model::schematic::{Frame, Paper};
    let Some(frame) = ws.schematic().map(|s| s.frame.clone()) else { return };
    let editable = ws.editable();
    if frame.is_none() && !editable {
        return;
    }
    section(ui, "Sheet");
    let set = |ws: &mut Workspace, frame: Option<Frame>| {
        let _ = ws.edit(|_, _, d| {
            if let Some(s) = d.schematic.as_mut() {
                s.frame = frame.clone();
            }
            Ok(())
        });
    };
    // Fitting the frame around what is drawn, ignoring the frame itself.
    let fit = |ws: &Workspace, mut frame: Frame| {
        let mut r = circuiter_scene::Rect::EMPTY;
        for e in ws.scene.elements.iter().filter(|e| e.role != circuiter_scene::Role::Frame) {
            r.union(&e.shape.bounds());
        }
        if !r.is_empty() {
            let mm = circuiter_model::geom::Point::mm;
            crate::edit::fit_frame(&mut frame, mm(r.min.x, r.min.y), mm(r.max.x, r.max.y));
        }
        frame
    };
    let mut on = frame.is_some();
    if ui
        .add_enabled(editable, egui::Checkbox::new(&mut on, "Frame and title block"))
        .on_hover_text("The border, grid references and title block it is printed in")
        .changed()
    {
        let new = on.then(|| fit(ws, Frame::new(Paper::A4)));
        set(ws, new);
        return;
    }
    let Some(frame) = frame else { return };
    let key = format!("{}/frame", ws.current().cell);
    let mut changed: Option<Frame> = None;
    egui::Grid::new("frame").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label("Paper");
        ui.horizontal(|ui| {
            let mut paper = frame.paper;
            ui.add_enabled_ui(editable, |ui| {
                egui::ComboBox::from_id_salt("paper")
                    .selected_text(paper.name())
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        for p in Paper::ALL {
                            ui.selectable_value(&mut paper, p, p.name());
                        }
                    });
            });
            let mut portrait = frame.portrait;
            ui.add_enabled(editable, egui::Checkbox::new(&mut portrait, "Portrait"));
            if paper != frame.paper || portrait != frame.portrait {
                changed = Some(Frame { paper, portrait, ..frame.clone() });
            }
        });
        ui.end_row();
        type FrameText = fn(&mut Frame) -> &mut String;
        let rows: [(&str, &str, FrameText); 5] = [
            ("Title", "the cell's name", |f| &mut f.title),
            ("Revision", "A", |f| &mut f.revision),
            ("Date", "2026-10-21", |f| &mut f.date),
            ("Company", "", |f| &mut f.company),
            ("Comment", "", |f| &mut f.comment),
        ];
        for (label, hint, get) in rows {
            ui.label(label);
            let mut copy = frame.clone();
            let current = get(&mut copy).clone();
            if !editable {
                ui.label(&current);
            } else if let Some(text) =
                field.edit_hinted(ui, &format!("{key}/{label}"), &current, 170.0, hint)
            {
                *get(&mut copy) = text.trim().to_owned();
                changed = Some(copy);
            }
            ui.end_row();
        }
    });
    if editable
        && ui
            .button("Fit Around Drawing")
            .on_hover_text("The smallest paper the sheet fits on, centred on what is drawn")
            .clicked()
    {
        changed = Some(fit(ws, frame.clone()));
    }
    if let Some(f) = changed {
        set(ws, Some(f));
    }
}

fn graphic_panel(ui: &mut egui::Ui, ws: &mut Workspace, index: usize, field: &mut Field) {
    let Some(graphic) = ws.symbol().and_then(|s| s.graphics.get(index)).cloned() else { return };
    let editable = ws.editable();
    let key = format!("{}/graphic/{index}", ws.current().cell);
    ui.label(RichText::new("Drawing").weak());
    let name = graphic_name(&graphic);
    let mut title = name.to_owned();
    title[..1].make_ascii_uppercase();
    ui.label(RichText::new(title).size(22.0).strong());
    section(ui, "Properties");

    let mut change: Option<GraphicChange> = None;
    let length_row = |ui: &mut egui::Ui,
                      field: &mut Field,
                      label: &str,
                      k: &str,
                      value: Length,
                      set: fn(&mut Graphic, Length)|
     -> Option<GraphicChange> {
        ui.label(label);
        let k = format!("{key}/{k}");
        let mut out: Option<GraphicChange> = None;
        if !editable {
            ui.label(format!("{} mm", value.mm()));
        } else if let Some(text) = field.edit(ui, &k, &format!("{}", value.mm()), 70.0) {
            match parse_length(&text) {
                Ok(len) => {
                    field.error = None;
                    out = Some(Box::new(move |g| set(g, len)));
                }
                Err(e) => field.error = Some((k, e)),
            }
        }
        ui.end_row();
        out
    };
    egui::Grid::new("graphic").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        let closed = match &graphic {
            Graphic::Polyline { points, .. } => points.len() > 2 && points.first() == points.last(),
            Graphic::Rect { .. } | Graphic::Circle { .. } => true,
            Graphic::Arc { .. } | Graphic::Text { .. } => false,
        };
        match &graphic {
            Graphic::Polyline { points, .. } => {
                ui.label("Points");
                ui.label(points.len().to_string());
                ui.end_row();
            }
            Graphic::Circle { radius, .. } => {
                change =
                    change.take().or(length_row(ui, field, "Radius", "radius", *radius, |g, r| {
                        if let Graphic::Circle { radius, .. } = g {
                            *radius = r;
                        }
                    }));
            }
            Graphic::Arc { radius, start, end, .. } => {
                change =
                    change.take().or(length_row(ui, field, "Radius", "radius", *radius, |g, r| {
                        if let Graphic::Arc { radius, .. } = g {
                            *radius = r;
                        }
                    }));
                for (label, value, is_start) in [("Start °", *start, true), ("End °", *end, false)]
                {
                    ui.label(label);
                    let k = format!("{key}/{label}");
                    if !editable {
                        ui.label(value.to_string());
                    } else if let Some(text) = field.edit(ui, &k, &value.to_string(), 70.0) {
                        match parse_degrees(&text) {
                            Ok(a) => {
                                field.error = None;
                                change = Some(Box::new(move |g| {
                                    if let Graphic::Arc { start, end, .. } = g {
                                        *(if is_start { start } else { end }) = a;
                                    }
                                }));
                            }
                            Err(e) => field.error = Some((k, e)),
                        }
                    }
                    ui.end_row();
                }
            }
            Graphic::Text { text, size, anchor, .. } => {
                ui.label("Text");
                let k = format!("{key}/text");
                if !editable {
                    ui.label(RichText::new(text).monospace());
                } else if let Some(new) = field.edit(ui, &k, text, 160.0) {
                    if new.trim().is_empty() {
                        field.error = Some((k, "text can't be empty; delete it instead".into()));
                    } else {
                        field.error = None;
                        change = Some(Box::new(move |g| {
                            if let Graphic::Text { text, .. } = g {
                                *text = new;
                            }
                        }));
                    }
                }
                ui.end_row();
                change = change.take().or(length_row(ui, field, "Size", "size", *size, |g, s| {
                    if let Graphic::Text { size, .. } = g {
                        *size = s;
                    }
                }));
                ui.label("Align");
                let mut a = *anchor;
                ui.add_enabled_ui(editable, |ui| {
                    egui::ComboBox::from_id_salt("anchor").selected_text(format!("{a:?}")).show_ui(
                        ui,
                        |ui| {
                            for x in [Anchor::Left, Anchor::Center, Anchor::Right] {
                                ui.selectable_value(&mut a, x, format!("{x:?}"));
                            }
                        },
                    );
                });
                if a != *anchor {
                    change = Some(Box::new(move |g| {
                        if let Graphic::Text { anchor, .. } = g {
                            *anchor = a;
                        }
                    }));
                }
                ui.end_row();
            }
            Graphic::Rect { .. } => {}
        }
        if closed {
            let filled = match &graphic {
                Graphic::Polyline { fill, .. }
                | Graphic::Rect { fill, .. }
                | Graphic::Circle { fill, .. } => *fill,
                _ => false,
            };
            let mut f = filled;
            ui.label("");
            if ui.add_enabled(editable, egui::Checkbox::new(&mut f, "Filled")).changed() {
                change = Some(Box::new(move |g| match g {
                    Graphic::Polyline { fill, .. }
                    | Graphic::Rect { fill, .. }
                    | Graphic::Circle { fill, .. } => *fill = f,
                    _ => {}
                }));
            }
            ui.end_row();
        }
    });
    if let Graphic::Text { .. } = graphic {
        ui.add_space(6.0);
        ui.label(
            RichText::new("${name} shows the reference, ${param.X} a parameter's value.")
                .weak()
                .small(),
        );
    }
    if let Some(f) = change {
        let _ = ws.edit(|_, _, d| {
            let g = d.symbol.as_mut().and_then(|s| s.graphics.get_mut(index)).ok_or("gone")?;
            f(g);
            Ok(())
        });
    }
}

impl eframe::App for CircuiterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.handle_screenshot(&ctx);
        self.handle_close_request(&ctx);
        self.prompts(&ctx);
        self.shortcuts(&ctx);
        footprint::window(self, &ctx);
        self.live_frame(&ctx);
        self.update_title(&ctx);
        self.take_disk_changes(&ctx);
        self.sync_space();
        if let Some(ws) = &self.workspace {
            // Project cells' symbols change as they are edited; std's never do.
            if ws.revision != self.icons_revision {
                self.icons.retain(|id, _| id.library == circuiter_model::STD_LIBRARY);
                self.icons_revision = ws.revision;
            }
        }

        let p = style::palette_for(self.theme.dark);
        let bar = |fill| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(0, 0));
        egui::Panel::top("titlebar").frame(bar(p.base)).show(ui, |ui| self.title_bar(ui));
        egui::Panel::top("toolbar").frame(bar(p.raised)).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(p.raised).inner_margin(egui::Margin::symmetric(10, 3)))
            .show(ui, |ui| self.status_bar(ui));
        let running =
            self.workspace.as_mut().and_then(|ws| ws.sim.as_mut()).is_some_and(|s| s.poll());
        let checking = self.workspace.as_mut().is_some_and(|ws| ws.poll_erc());
        if running || checking {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.workspace.is_some() {
            let panel = egui::Frame::new().fill(p.panel);
            let space = self.space.name();
            egui::Panel::left(format!("explorer-{space}"))
                .resizable(true)
                .default_size(270.0)
                .frame(panel)
                .show(ui, |ui| {
                    // Panels keep their size whatever their contents.
                    ui.expand_to_include_rect(ui.max_rect());
                    self.explorer(ui)
                });
            egui::Panel::right(format!("inspector-{space}"))
                .resizable(true)
                .default_size(360.0)
                .frame(panel)
                .show(ui, |ui| {
                    ui.expand_to_include_rect(ui.max_rect());
                    self.right_panel(ui)
                });
            // Results have nothing to show about a 3D body.
            if self.show_bottom && !self.show_3d {
                egui::Panel::bottom(format!("results-{space}"))
                    .resizable(true)
                    .default_size(280.0)
                    .frame(panel)
                    .show(ui, |ui| {
                        ui.expand_to_include_rect(ui.max_rect());
                        self.bottom_panel(ui)
                    });
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.workspace.is_some() {
                egui::Panel::top("doc-tabs")
                    .frame(egui::Frame::new().fill(p.raised).inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: 0,
                    }))
                    .show(ui, |ui| self.doc_tabs(ui));
                if self.workspace.as_ref().is_some_and(|ws| ws.live.is_some()) {
                    egui::Panel::top("live-bar")
                        .frame(egui::Frame::new().fill(p.error.gamma_multiply(0.12)))
                        .show(ui, |ui| self.live_bar(ui));
                }
                if self.active_code.is_none() {
                    egui::Panel::top("canvas-header")
                        .frame(egui::Frame::new().fill(p.panel))
                        .show(ui, |ui| self.canvas_header(ui));
                }
            }
            if self.active_code.is_some() && self.workspace.is_some() {
                self.cursor = None;
                self.hovered = None;
                self.code_view(ui);
                return;
            }
            let theme = self.theme;
            let show_grid = self.show_grid;
            let netlist = self.show_netlist
                && self.workspace.as_ref().is_some_and(|ws| ws.has_schematic());
            let diagram = self.show_diagram
                && self.workspace.as_ref().is_some_and(|ws| ws.has_schematic());
            if self.show_3d && self.workspace.is_some() {
                self.cursor = None;
                self.hovered = None;
                self.view_3d(ui);
                return;
            }
            if self.show_pcb && self.workspace.is_some() {
                self.cursor = None;
                self.hovered = None;
                self.view_pcb(ui);
                self.error_toast(&ui.ctx().clone(), ui.max_rect());
                return;
            }
            match &mut self.workspace {
                None => {
                    self.cursor = None;
                    self.hovered = None;
                    egui::Frame::new().fill(p.base).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.welcome(ui);
                    });
                }
                Some(_) if diagram => {
                    self.show_netlist = false;
                    self.diagram_view(ui);
                    self.error_toast(&ui.ctx().clone(), ui.max_rect());
                }
                Some(_) if netlist => {
                    self.cursor = None;
                    self.hovered = None;
                    self.netlist_view(ui);
                }
                Some(ws) => {
                    self.show_netlist = false;
                    self.show_diagram = false;
                    if ws.editable() && ws.mode == Mode::Schematic {
                        egui::Panel::left("parts-column")
                            .resizable(false)
                            .exact_size(84.0)
                            .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(4, 0)))
                            .show(ui, |ui| self.parts_column(ui));
                    }
                    let Some(ws) = &mut self.workspace else { return };
                    let canvas_rect = ui.available_rect_before_wrap();
                    let overlay = ws.overlay(self.cursor);
                    let selection = ws.selection.clone();
                    let highlight = Highlight { selection: Some(&selection), net: ws.net };
                    let level = ws.levels.last_mut().expect("level");
                    self.canvas.hand = ws.tool == Tool::Pan;
                    let out = canvas::show(
                        ui,
                        &ws.scene,
                        &mut level.camera,
                        &mut self.canvas,
                        theme,
                        highlight,
                        &overlay,
                        show_grid,
                    );
                    if canvas_rect.width() > 340.0 && canvas_rect.height() > 160.0 {
                        style::canvas_badge(
                            &ui.painter_at(canvas_rect),
                            canvas_rect.left_top() + egui::vec2(14.0, 14.0),
                            egui::Align2::LEFT_TOP,
                            &format!("{}  ·  {}", ws.current().cell.cell, ws.tool.name()),
                            style::palette_for(theme.dark),
                            p.accent,
                        );
                    }
                    self.cursor = out.cursor;
                    self.hovered = out.hovered;
                    ws.pick_tolerance = out.tolerance;
                    let ctx = ui.ctx().clone();
                    let (cursor, tolerance) = (out.cursor, out.tolerance);
                    let gestures = self.live_interact(&ctx, out.gestures);
                    self.drop_part(&ctx, cursor, tolerance);
                    let Some(ws) = &mut self.workspace else { return };
                    let empty_sheet = ws.schematic().is_some_and(|s| {
                        s.instances.is_empty() && s.wires.is_empty() && s.ports.is_empty()
                    });
                    if empty_sheet && ws.editable() && !matches!(ws.tool, Tool::Place { .. }) {
                        empty_sheet_guide(ui, canvas_rect, theme);
                    }
                    if ws.mode == Mode::Symbol && ws.symbol().is_none() && ws.tool == Tool::Select {
                        let text = if ws.editable() {
                            "No symbol yet.\nAdd a port in the inspector and one is generated,\nor draw one with the tools above."
                        } else {
                            "This cell has no symbol."
                        };
                        ui.painter().text(
                            canvas_rect.center() + egui::vec2(0.0, 80.0),
                            egui::Align2::CENTER_TOP,
                            text,
                            egui::FontId::proportional(15.0),
                            canvas::color(theme.pin_name),
                        );
                    }
                    for g in gestures {
                        if let Some(err) = ws.gesture(g, tolerance) {
                            self.error = Some(err);
                        }
                    }
                    // Right-click a wire, label or port for what can be done with its net.
                    if let Some(response) = &out.response {
                        if response.secondary_clicked() {
                            self.net_menu = self.hovered.and_then(|o| ws.net_of(o));
                            if self.net_menu.is_some() {
                                ws.select_only(self.hovered);
                            }
                        }
                        if let Some(net) = self.net_menu {
                            if response.context_menu(|ui| net_menu(ui, ws, net)).is_none() {
                                self.net_menu = None;
                            }
                        }
                    }
                    self.error_toast(&ctx, canvas_rect);
                }
            }
        });
    }
}

impl CircuiterApp {
    /// The workspace in front, as the views showing make it.
    pub(super) fn current_space(&self) -> Space {
        match (self.show_pcb, self.show_3d, self.simulating) {
            (true, _, _) => Space::Pcb,
            (_, true, _) => Space::Model,
            (_, _, true) => Space::Simulation,
            _ => Space::Schematic,
        }
    }

    /// Brings `space` to the front.
    pub(super) fn set_space(&mut self, space: Space) {
        (self.show_pcb, self.show_3d, self.simulating) = match space {
            Space::Schematic => (false, false, false),
            Space::Pcb => (true, false, false),
            Space::Model => (false, true, false),
            Space::Simulation => (false, false, true),
        };
        self.show_netlist = false;
        self.show_diagram = false;
        self.active_code = None;
        if matches!(space, Space::Schematic | Space::Simulation) {
            if let Some(ws) = self.workspace.as_mut().filter(|ws| ws.has_schematic()) {
                if space == Space::Simulation || ws.mode != Mode::Symbol {
                    ws.set_mode(Mode::Schematic);
                }
            }
        }
        self.sync_space();
    }

    /// Swaps the panels when the workspace in front has changed, however it changed;
    /// a run started on the schematic brings the Simulation workspace forward.
    fn sync_space(&mut self) {
        let running = self.workspace.as_ref().is_some_and(|ws| {
            ws.live.is_some() || ws.sim.as_ref().is_some_and(|s| s.job.is_some())
        });
        if running && !self.was_running && self.current_space() == Space::Schematic {
            self.simulating = true;
            self.show_netlist = false;
            self.show_diagram = false;
        }
        self.was_running = running;
        let now = self.current_space();
        if now == self.space {
            return;
        }
        self.spaces[self.space.index()] = Panels {
            left_tab: self.left_tab,
            right_tab: self.right_tab,
            bottom_tab: self.bottom_tab,
            show_bottom: self.show_bottom,
        };
        let p = self.spaces[now.index()];
        (self.left_tab, self.right_tab, self.bottom_tab, self.show_bottom) =
            (p.left_tab, p.right_tab, p.bottom_tab, p.show_bottom);
        self.space = now;
    }

    /// The latest error, over the bottom of the sheet until dismissed.
    fn error_toast(&mut self, ctx: &egui::Context, sheet: egui::Rect) {
        let Some(err) = self.error.clone() else { return };
        let p = style::palette_for(self.theme.dark);
        egui::Area::new(egui::Id::new("error-toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
            .constrain_to(sheet)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.raised)
                    .stroke(egui::Stroke::new(1.0, p.error.gamma_multiply(0.7)))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(sheet.width() * 0.8);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(style::icon::WARNING_CIRCLE).color(p.error));
                            ui.label(RichText::new(&err).color(p.text));
                            let x = egui::Button::new(RichText::new(style::icon::X).color(p.weak))
                                .frame(false);
                            if ui.add(x).on_hover_text("Dismiss").clicked() {
                                self.error = None;
                            }
                        });
                    });
            });
    }
}
