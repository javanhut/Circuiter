//! An open project being viewed and edited: navigation, selection, tools, undo and save.
//!
//! Everything here is independent of the GUI toolkit. The app feeds in canvas gestures
//! and key commands, and draws from the scene and the overlay this produces.

use crate::canvas::{Camera, Gesture, Overlay};
use crate::edit::{self, GRID};
use crate::interface;
use crate::simulation::{self, Results, Simulation};
use crate::symbol_edit::{self, FINE_GRID};
use anyhow::{Context as _, Result};
use circuiter_model::builder;
use circuiter_model::cell::{Cell, PortKind, validate_name};
use circuiter_model::check::{Diagnostic, Severity, check_libraries};
use circuiter_model::connectivity::Item;
use circuiter_model::geom::{Length, Point, Rotation, Transform};
use circuiter_model::schematic::{Instance, Schematic};
use circuiter_model::symbol::{Anchor, Graphic, PinDirection, Symbol};
use circuiter_model::{CellDef, CellId, CellRef, Design, Project, STD_LIBRARY};
use circuiter_scene::{
    Element, HAlign, Owner, Pt, Rect, Role, Scene, Shape, VAlign, instance_elements,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const HISTORY_LIMIT: usize = 200;

/// Which view of the current cell the canvas shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Schematic,
    Symbol,
}

/// One step of the hierarchy path being viewed.
#[derive(Debug, Clone)]
pub struct Level {
    pub cell: CellId,
    /// The instance in the parent level that was descended into.
    pub via: Option<String>,
    pub camera: Option<Camera>,
}

/// The active editing tool.
#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Select,
    Place { cell: CellRef, rotation: Rotation, mirror: bool },
    Wire { points: Vec<Point>, horizontal_first: bool },
    Label { net: String },
    Port { name: String, kind: PortKind },
    // Symbol drawing.
    Line { points: Vec<Point> },
    Rect { from: Option<Point> },
    Circle { center: Option<Point> },
    Arc { center: Option<Point>, start: Option<Point> },
    Text { text: String },
    Pin { port: String, direction: PinDirection },
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Place { .. } => "Place",
            Tool::Wire { .. } => "Wire",
            Tool::Label { .. } => "Label",
            Tool::Port { .. } => "Port",
            Tool::Line { .. } => "Line",
            Tool::Rect { .. } => "Rectangle",
            Tool::Circle { .. } => "Circle",
            Tool::Arc { .. } => "Arc",
            Tool::Text { .. } => "Text",
            Tool::Pin { .. } => "Pin",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Tool::Select => {
                "Click to select · drag to move · drag from a pin's end to wire it · drag empty space to box-select · double-click to open"
            }
            Tool::Place { .. } => {
                "Click to place (Shift+click to place another) · R rotate · X mirror · Esc cancel"
            }
            Tool::Wire { points, .. } if points.is_empty() => "Click to start a wire · Esc done",
            Tool::Wire { .. } => {
                "Click to add a corner · click a pin or wire to finish · double-click to end · / flip corner · Esc cancel"
            }
            Tool::Label { .. } => "Click a wire or pin to label its net · Esc done",
            Tool::Port { .. } => "Click where the port connects · Esc cancel",
            Tool::Line { points } if points.is_empty() => "Click to start a line · Esc done",
            Tool::Line { .. } => {
                "Click to add a point · click the start to close · double-click to end · Esc cancel"
            }
            Tool::Rect { from: None } => "Click the first corner · Esc done",
            Tool::Rect { .. } => "Click the opposite corner · Esc cancel",
            Tool::Circle { center: None } => "Click the centre · Esc done",
            Tool::Circle { .. } => "Click to set the radius · Esc cancel",
            Tool::Arc { center: None, .. } => "Click the centre · Esc done",
            Tool::Arc { start: None, .. } => "Click where the arc starts · Esc cancel",
            Tool::Arc { .. } => "Click where it ends, counter-clockwise · Esc cancel",
            Tool::Text { .. } => "Click to place the text · Esc cancel",
            Tool::Pin { .. } => "Click where wires connect · R rotate · X flip · Esc cancel",
        }
    }

    /// Whether this tool draws on symbols rather than schematics.
    pub fn for_symbols(&self) -> bool {
        matches!(
            self,
            Tool::Line { .. }
                | Tool::Rect { .. }
                | Tool::Circle { .. }
                | Tool::Arc { .. }
                | Tool::Text { .. }
                | Tool::Pin { .. }
        )
    }
}

#[derive(Debug)]
enum Drag {
    /// Moving objects on `grid`; `base` is the cell before the move started.
    /// `grab` is where the pointer took hold, unsnapped: which segment of a wire.
    Move { base: Box<CellDef>, from: Point, grab: Point, grid: Length, targets: BTreeSet<Owner> },
    /// Rubber-band selection.
    Band { from: Pt },
    /// Drawing a wire by dragging it out of a pin: let go to end it.
    Wire,
}

#[derive(Debug, Default)]
struct History {
    undo: Vec<CellDef>,
    redo: Vec<CellDef>,
}

#[derive(Debug)]
pub struct Workspace {
    pub project: Project,
    pub design: Design,
    pub diagnostics: Vec<Diagnostic>,
    pub levels: Vec<Level>,
    pub mode: Mode,
    pub scene: Scene,
    pub selection: BTreeSet<Owner>,
    /// The net under inspection (index into the scene's nets).
    pub net: Option<usize>,
    pub tool: Tool,
    /// Unsaved changes exist.
    pub dirty: bool,
    drag: Option<Drag>,
    history: BTreeMap<CellId, History>,
    /// The latest simulation of the design being viewed.
    pub sim: Option<Simulation>,
    /// Why the design couldn't be simulated at all.
    pub sim_error: Option<String>,
    /// What the last edit connected or disconnected, for the status bar.
    pub notice: Option<String>,
    /// How close, in mm, the pointer must be to pick something, at the current zoom.
    /// Kept up to date by the canvas, for hover feedback.
    pub pick_tolerance: f64,
    /// The wire being drawn was started from a pin in Select: finishing (or
    /// cancelling) it returns to Select.
    wire_from_pin: bool,
    /// What Copy last took. Copy also puts it on the system clipboard as text; this is
    /// for when that can't be read back.
    clipboard: Option<edit::Clip>,
    /// Counts changes to the design, so views derived from it know when to refresh.
    pub revision: u64,
    /// The circuit running live, if one is.
    pub live: Option<crate::live::Session>,
    /// The design revision the live run started from: edits since make it out of date.
    pub live_revision: u64,
    /// The operated parts' states the scene was last drawn with.
    live_drawn: BTreeMap<String, String>,
    /// Flat nets of the running circuit shown in the live waveforms.
    pub live_watch: Vec<usize>,
    /// The last run plan worked out, for which revision and top cell.
    plan_cache: std::cell::RefCell<Option<(u64, CellId, simulation::RunPlan)>>,
    /// Open tabs, each a cell and how it was left. The active tab's live state is
    /// `levels` and `mode`; its entry here is brought up to date when another is shown.
    tabs: Vec<View>,
    active_tab: usize,
    /// Where Back and Forward go: earlier and later views, most recent last.
    back: Vec<View>,
    forward: Vec<View>,
}

/// A place visited: the hierarchy path, which view of it, and where the camera was.
#[derive(Debug, Clone)]
struct View {
    levels: Vec<Level>,
    mode: Mode,
}

/// How many views Back remembers.
const BACK_LIMIT: usize = 100;

pub use crate::simulation::Analysis;

/// A part's outline and how close it came to destruction, from 0 (ambient) to 1.
type Heat = (Rect, f32);

/// The outcome of importing one model: the new cell and why it can't be simulated yet
/// (if it can't), or why it couldn't be imported.
pub type ImportOutcome = Result<(String, Option<String>), String>;

pub fn to_point(p: Pt) -> Point {
    Point::mm(p.x, p.y)
}

fn snapped(p: Pt) -> Point {
    edit::snap(to_point(p), GRID)
}

fn snapped_fine(p: Pt) -> Point {
    edit::snap(to_point(p), FINE_GRID)
}

fn rotate_direction(d: PinDirection) -> PinDirection {
    match d {
        PinDirection::Right => PinDirection::Up,
        PinDirection::Up => PinDirection::Left,
        PinDirection::Left => PinDirection::Down,
        PinDirection::Down => PinDirection::Right,
    }
}

impl Workspace {
    pub fn open(path: &Path) -> Result<Workspace> {
        let root = Project::find_root(path)
            .with_context(|| format!("no Circuiter project found at {}", path.display()))?;
        let project = Project::load(&root)?;
        let design = Design::for_project(&project, circuiter_stdlib::load()?)?;
        let top = CellId::new(project.name(), &project.manifest.project.top);
        design.cell(&top).with_context(|| format!("top cell {top} not found"))?;
        let scene = Scene::symbol_sheet(&design, &top).expect("cell exists");
        let mut ws = Workspace {
            project,
            design,
            diagnostics: Vec::new(),
            levels: vec![Level { cell: top, via: None, camera: None }],
            mode: Mode::Symbol,
            scene,
            selection: BTreeSet::new(),
            net: None,
            tool: Tool::Select,
            dirty: false,
            drag: None,
            history: BTreeMap::new(),
            sim: None,
            sim_error: None,
            notice: None,
            pick_tolerance: 0.5,
            wire_from_pin: false,
            clipboard: None,
            revision: 0,
            live: None,
            live_revision: 0,
            live_drawn: BTreeMap::new(),
            live_watch: Vec::new(),
            plan_cache: std::cell::RefCell::new(None),
            tabs: Vec::new(),
            active_tab: 0,
            back: Vec::new(),
            forward: Vec::new(),
        };
        ws.mode = ws.default_mode();
        ws.tabs.push(View { levels: ws.levels.clone(), mode: ws.mode });
        ws.rebuild_scene();
        ws.recheck();
        Ok(ws)
    }

    /// Creates a project in `dir`, named after the folder, and opens it.
    pub fn create(dir: &Path) -> Result<Workspace> {
        let base = dir.file_name().and_then(|n| n.to_str()).context("the folder has no name")?;
        let mut name: String =
            base.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            name.insert_str(0, "p_");
        }
        Project::create(dir, &name)?;
        Workspace::open(dir)
    }

    pub fn current(&self) -> &Level {
        self.levels.last().expect("at least one level")
    }

    pub fn current_def(&self) -> Option<&CellDef> {
        self.design.cell(&self.current().cell)
    }

    /// The schematic on the canvas, when the schematic view is showing.
    pub fn schematic(&self) -> Option<&Schematic> {
        match self.mode {
            Mode::Schematic => self.current_def()?.schematic.as_ref(),
            Mode::Symbol => None,
        }
    }

    /// The symbol on the canvas, when the symbol view is showing.
    pub fn symbol(&self) -> Option<&Symbol> {
        match self.mode {
            Mode::Symbol => self.current_def()?.symbol.as_ref(),
            Mode::Schematic => None,
        }
    }

    pub fn has_schematic(&self) -> bool {
        self.current_def().is_some_and(|d| d.schematic.is_some())
    }

    /// The view to show when arriving at a cell: its schematic if it has one.
    fn default_mode(&self) -> Mode {
        if self.has_schematic() { Mode::Schematic } else { Mode::Symbol }
    }

    /// Switches between the current cell's schematic and symbol.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode || (mode == Mode::Schematic && !self.has_schematic()) {
            return;
        }
        if let Some(Drag::Move { .. }) = self.drag {
            self.escape();
        }
        self.mode = mode;
        self.levels.last_mut().expect("level").camera = None;
        self.rebuild_scene();
    }

    /// Whether the current cell can be edited. The standard library is read-only.
    pub fn editable(&self) -> bool {
        self.current().cell.library != STD_LIBRARY
    }

    /// Libraries whose problems are reported: everything except the built-in one.
    fn checked_libraries(&self) -> Vec<String> {
        let mut libs = vec![self.project.name().to_owned()];
        libs.extend(self.project.manifest.libraries.keys().cloned());
        libs
    }

    fn recheck(&mut self) {
        let libs = self.checked_libraries();
        let refs: Vec<&str> = libs.iter().map(String::as_str).collect();
        self.diagnostics = check_libraries(&self.design, &refs);
    }

    fn build_scene(&self) -> Option<Scene> {
        let id = &self.current().cell;
        match self.mode {
            Mode::Schematic if self.live.is_some() => {
                // Switches and buttons drawn as they are in the running circuit.
                let def = self.design.cell(id)?;
                let mut schematic = def.schematic.clone()?;
                let states = self.live_states();
                let sheet = self.sheet_path();
                for inst in &mut schematic.instances {
                    let path = if sheet.is_empty() {
                        inst.name.clone()
                    } else {
                        format!("{sheet}.{}", inst.name)
                    };
                    if let (Some(value), Some(kind)) =
                        (states.get(&path), self.live.as_ref().and_then(|l| l.control(&path)))
                    {
                        inst.params.insert(kind.param().to_owned(), value.clone());
                    }
                }
                Some(Scene::build_from(&self.design, id, &def.cell, &schematic))
            }
            Mode::Schematic => Scene::build(&self.design, id),
            Mode::Symbol => Scene::symbol_sheet(&self.design, id),
        }
    }

    /// Rebuilds the scene after an edit, keeping the selection where it is still valid.
    fn refresh(&mut self) {
        if let Some(scene) = self.build_scene() {
            self.scene = scene;
        }
        let (n_inst, n_wire, n_label, n_port) = self.schematic().map_or((0, 0, 0, 0), |s| {
            (s.instances.len(), s.wires.len(), s.labels.len(), s.ports.len())
        });
        let (n_graphic, n_pin) = self.symbol().map_or((0, 0), |s| (s.graphics.len(), s.pins.len()));
        self.selection.retain(|o| match *o {
            Owner::Instance(i) => i < n_inst,
            Owner::Wire(i) => i < n_wire,
            Owner::Label(i) => i < n_label,
            Owner::Port(i) => i < n_port,
            Owner::Graphic(i) => i < n_graphic,
            Owner::Pin(i) => i < n_pin,
            Owner::Derived => false,
        });
        self.update_net();
        self.recheck();
    }

    /// Rebuilds the scene after navigating, resetting selection and tool.
    fn rebuild_scene(&mut self) {
        if self.mode == Mode::Schematic && !self.has_schematic() {
            self.mode = Mode::Symbol;
        }
        if let Some(scene) = self.build_scene() {
            self.scene = scene;
        }
        self.selection.clear();
        self.net = None;
        self.tool = Tool::Select;
        self.drag = None;
        self.wire_from_pin = false;
        self.notice = None;
    }

    /// Remembers the current view for Back, before going somewhere else.
    fn remember(&mut self) {
        let here = View { levels: self.levels.clone(), mode: self.mode };
        self.back.push(here);
        if self.back.len() > BACK_LIMIT {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// The cell Back would return to, if any.
    pub fn back_to(&self) -> Option<&CellId> {
        self.back.last().map(|v| &v.levels.last().expect("level").cell)
    }

    pub fn forward_to(&self) -> Option<&CellId> {
        self.forward.last().map(|v| &v.levels.last().expect("level").cell)
    }

    /// Returns to the previous view (`Alt+←`), as it was left: same view, same zoom.
    pub fn go_back(&mut self) {
        self.travel(true);
    }

    pub fn go_forward(&mut self) {
        self.travel(false);
    }

    fn travel(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        let Some(view) = from.pop() else { return };
        to.push(View { levels: self.levels.clone(), mode: self.mode });
        // Back into the tab the view belongs to, reopening it if it was closed.
        let root = &view.levels[0].cell;
        match self.tabs.iter().position(|t| t.levels[0].cell == *root) {
            Some(i) => self.set_active_tab(i),
            None => {
                self.store_tab();
                self.tabs.push(view.clone());
                self.active_tab = self.tabs.len() - 1;
            }
        }
        self.load_view(view);
    }

    /// Shows `view`, with the camera where it was left.
    fn load_view(&mut self, view: View) {
        self.levels = view.levels;
        self.mode = view.mode;
        let camera = self.current().camera;
        self.rebuild_scene();
        self.levels.last_mut().expect("level").camera = camera;
    }

    /// Keeps the active tab's view in its tab, before showing another.
    fn store_tab(&mut self) {
        self.tabs[self.active_tab] = View { levels: self.levels.clone(), mode: self.mode };
    }

    fn set_active_tab(&mut self, i: usize) {
        if i != self.active_tab {
            self.store_tab();
            self.active_tab = i;
        }
    }

    /// The open tabs: for each, the cell it shows. The active one is
    /// [`Workspace::active_tab`].
    pub fn tabs(&self) -> Vec<CellId> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let levels = if i == self.active_tab { &self.levels } else { &t.levels };
                levels.last().expect("level").cell.clone()
            })
            .collect()
    }

    pub fn active_tab(&self) -> usize {
        self.active_tab
    }

    /// Brings tab `i` to the front, as it was left.
    pub fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tabs.len() {
            return;
        }
        self.remember();
        self.set_active_tab(i);
        let view = self.tabs[i].clone();
        self.load_view(view);
    }

    /// Closes tab `i`; the last tab stays open.
    pub fn close_tab(&mut self, i: usize) {
        if self.tabs.len() < 2 || i >= self.tabs.len() {
            return;
        }
        if i == self.active_tab {
            self.remember();
            self.tabs.remove(i);
            self.active_tab = i.saturating_sub(1).min(self.tabs.len() - 1);
            let view = self.tabs[self.active_tab].clone();
            self.load_view(view);
        } else {
            self.tabs.remove(i);
            if i < self.active_tab {
                self.active_tab -= 1;
            }
        }
    }

    /// Shows a cell: in its tab if it has one (back at its top level), otherwise in a
    /// new tab.
    pub fn open_cell(&mut self, id: CellId) {
        let fresh = |ws: &Workspace| {
            let levels = vec![Level { cell: id.clone(), via: None, camera: None }];
            let mode = ws.design.cell(&id).map_or(Mode::Symbol, |d| {
                if d.schematic.is_some() { Mode::Schematic } else { Mode::Symbol }
            });
            View { levels, mode }
        };
        match self.tabs.iter().position(|t| t.levels[0].cell == id) {
            Some(i) if i == self.active_tab => {
                if self.levels.len() > 1 {
                    self.remember();
                    let view = fresh(self);
                    self.load_view(view);
                }
            }
            Some(i) => self.switch_tab(i),
            None => {
                self.remember();
                self.store_tab();
                let view = fresh(self);
                self.tabs.push(view.clone());
                self.active_tab = self.tabs.len() - 1;
                self.load_view(view);
            }
        }
    }

    pub fn descend(&mut self, instance: usize) {
        let Some(schematic) = self.schematic() else { return };
        let Some(inst) = schematic.instances.get(instance) else { return };
        let Some((child, _)) = self.design.resolve(&inst.cell, &self.current().cell.library) else {
            return;
        };
        let via = Some(inst.name.clone());
        self.remember();
        self.levels.push(Level { cell: child, via, camera: None });
        self.mode = self.default_mode();
        self.rebuild_scene();
    }

    pub fn ascend_to(&mut self, depth: usize) {
        if depth == 0 || depth >= self.levels.len() {
            return;
        }
        let child = self.levels[depth].via.clone();
        self.remember();
        self.levels.truncate(depth);
        self.mode = Mode::Schematic;
        self.rebuild_scene();
        // Keep the instance we came out of selected, as a landmark.
        if let (Some(name), Some(schematic)) = (child, self.schematic()) {
            let found = schematic.instances.iter().position(|i| i.name == name);
            self.select_only(found.map(Owner::Instance));
        }
    }

    pub fn select_only(&mut self, owner: Option<Owner>) {
        self.selection = owner.into_iter().filter(|o| *o != Owner::Derived).collect();
        self.update_net();
    }

    pub fn select_net(&mut self, net: usize) {
        let first = self.scene.connectivity.nets.get(net).and_then(|n| {
            n.items.iter().find_map(|it| match *it {
                Item::Wire(w) => Some(Owner::Wire(w)),
                Item::Label(l) => Some(Owner::Label(l)),
                Item::Port(p) => Some(Owner::Port(p)),
                Item::Pin(_) => None,
            })
        });
        self.selection = first.into_iter().collect();
        self.net = Some(net);
    }

    fn update_net(&mut self) {
        let conn = &self.scene.connectivity;
        let mut it = self.selection.iter();
        self.net = match (it.next(), it.next()) {
            (Some(Owner::Wire(i)), None) => conn.net_of(Item::Wire(*i)),
            (Some(Owner::Label(i)), None) => conn.net_of(Item::Label(*i)),
            (Some(Owner::Port(i)), None) => conn.net_of(Item::Port(*i)),
            _ => None,
        };
    }

    /// Applies an edit to the current cell as one undoable step. A generated symbol is
    /// kept in step with any change to the ports.
    pub fn edit<T>(
        &mut self,
        f: impl FnOnce(&Design, &str, &mut CellDef) -> Result<T, String>,
    ) -> Result<T, String> {
        if !self.editable() {
            return Err("this sheet is read-only".into());
        }
        let id = self.current().cell.clone();
        let before = self.design.cell(&id).cloned().ok_or("cell not found")?;
        let mut after = before.clone();
        let out = f(&self.design, &id.library, &mut after)?;
        builder::sync(&before, &mut after);
        if after != before {
            let groups = self.pin_groups(&before);
            self.record(&id, before);
            *self.design.cell_mut(&id).expect("exists") = after;
            self.dirty = true;
            self.refresh();
            self.note_changes(&groups);
        }
        Ok(out)
    }

    /// For each pin, label and port on `def`'s sheet, what else it is joined to.
    fn pin_groups(&self, def: &CellDef) -> BTreeMap<String, BTreeSet<String>> {
        let Some(s) = def.schematic.as_ref().filter(|_| self.mode == Mode::Schematic) else {
            return BTreeMap::new();
        };
        let conn = circuiter_model::connectivity::Connectivity::analyze(
            &self.design,
            &self.current().cell.library,
            s,
        );
        let mut out = BTreeMap::new();
        for net in &conn.nets {
            let members: BTreeSet<String> = net
                .items
                .iter()
                .filter_map(|item| match *item {
                    Item::Pin(k) => {
                        let pin = &conn.pins[k];
                        Some(format!("{}.{}", s.instances[pin.instance].name, pin.port))
                    }
                    Item::Label(k) => Some(s.labels[k].net.clone()),
                    Item::Port(k) => Some(format!("port {}", s.ports[k].port)),
                    Item::Wire(_) => None,
                })
                .collect();
            for m in &members {
                let mut others = members.clone();
                others.remove(m);
                out.insert(m.clone(), others);
            }
        }
        out
    }

    /// Sets the notice to what changed between `before` and the sheet now.
    fn note_changes(&mut self, before: &BTreeMap<String, BTreeSet<String>>) {
        let Some(def) = self.current_def().cloned() else { return };
        let after = self.pin_groups(&def);
        let pairs = |groups: &BTreeMap<String, BTreeSet<String>>| -> BTreeSet<(String, String)> {
            groups
                .iter()
                .flat_map(|(a, others)| {
                    others.iter().filter(move |b| a < *b).map(move |b| (a.clone(), b.clone()))
                })
                .collect()
        };
        let (old, new) = (pairs(before), pairs(&after));
        // Things that exist on both sides; a deleted part isn't "disconnected".
        let exists = |n: &str| before.contains_key(n) && after.contains_key(n);
        let list = |set: Vec<&(String, String)>| -> String {
            let mut parts: Vec<String> =
                set.iter().take(4).map(|(a, b)| format!("{a}–{b}")).collect();
            if set.len() > 4 {
                parts.push(format!("{} more", set.len() - 4));
            }
            parts.join(", ")
        };
        let joined: Vec<_> = new.difference(&old).filter(|(a, b)| exists(a) && exists(b)).collect();
        let parted: Vec<_> = old.difference(&new).filter(|(a, b)| exists(a) && exists(b)).collect();
        let mut text = Vec::new();
        if !joined.is_empty() {
            text.push(format!("Connected {}", list(joined)));
        }
        if !parted.is_empty() {
            text.push(format!("Disconnected {}", list(parted)));
        }
        self.notice = (!text.is_empty()).then(|| text.join(" · "));
    }

    fn record(&mut self, id: &CellId, before: CellDef) {
        self.mark_stale();
        let h = self.history.entry(id.clone()).or_default();
        h.undo.push(before);
        if h.undo.len() > HISTORY_LIMIT {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        self.history.get(&self.current().cell).is_some_and(|h| !h.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.history.get(&self.current().cell).is_some_and(|h| !h.redo.is_empty())
    }

    pub fn undo(&mut self) {
        self.step_history(true);
    }

    pub fn redo(&mut self) {
        self.step_history(false);
    }

    fn step_history(&mut self, undo: bool) {
        let id = self.current().cell.clone();
        let Some(h) = self.history.get_mut(&id) else { return };
        let Some(target) = (if undo { h.undo.pop() } else { h.redo.pop() }) else { return };
        let Some(cell) = self.design.cell_mut(&id) else { return };
        let previous = std::mem::replace(cell, target);
        if undo {
            h.redo.push(previous)
        } else {
            h.undo.push(previous)
        }
        self.mark_stale();
        self.dirty = true;
        self.drag = None;
        self.selection.clear();
        self.refresh();
    }

    /// Writes the project (and any external libraries) to disk.
    pub fn save(&mut self) -> Result<()> {
        let name = self.project.name().to_owned();
        self.project.library =
            self.design.library(&name).context("project library missing")?.clone();
        self.project.save()?;
        for (lib, entry) in &self.project.manifest.libraries {
            if let Some(l) = self.design.library(lib) {
                l.save_dir(&self.project.root.join(&entry.path))?;
            }
        }
        self.dirty = false;
        Ok(())
    }

    /// Creates an empty schematic cell in the project library, in `folder` (empty for
    /// the top level), and opens it.
    pub fn new_cell(&mut self, name: &str, folder: &str) -> Result<(), String> {
        validate_name(name)?;
        let folder = interface::clean_folder(folder);
        circuiter_model::validate_folder(&folder)?;
        let lib = self.project.name().to_owned();
        let library = self.design.library_mut(&lib).ok_or("project library missing")?;
        if library.cell(name).is_some() {
            return Err(format!("`{name}` already exists in {lib}"));
        }
        let path = if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") };
        if library.all_folders().contains(&path) {
            return Err(format!("there is a folder {path}/; pick another name"));
        }
        library.insert(CellDef {
            name: name.to_owned(),
            folder,
            cell: Cell::default(),
            schematic: Some(Schematic::default()),
            ..CellDef::default()
        });
        self.dirty = true;
        self.recheck();
        self.open_cell(CellId::new(&lib, name));
        Ok(())
    }

    /// Whether cells and folders of library `lib` can be changed: the built-in one
    /// can't.
    pub fn library_editable(&self, lib: &str) -> bool {
        lib != STD_LIBRARY && self.design.library(lib).is_some()
    }

    fn folder_library(&mut self, lib: &str) -> Result<&mut circuiter_model::Library, String> {
        if !self.library_editable(lib) {
            return Err(format!("{lib} is read-only"));
        }
        self.design.library_mut(lib).ok_or_else(|| format!("no library {lib}"))
    }

    /// Makes a folder, and any folders above it, in library `lib`. Returns its path.
    pub fn new_folder(&mut self, lib: &str, folder: &str) -> Result<String, String> {
        let folder = interface::clean_folder(folder);
        if folder.is_empty() {
            return Err("give the folder a name".into());
        }
        circuiter_model::validate_folder(&folder)?;
        let library = self.folder_library(lib)?;
        if library.all_folders().contains(&folder) {
            return Err(format!("{folder}/ already exists"));
        }
        clash(library, &folder)?;
        library.folders.insert(folder.clone());
        self.dirty = true;
        Ok(folder)
    }

    /// Renames or moves folder `old` to `new` (full paths), with everything in it.
    pub fn rename_folder(&mut self, lib: &str, old: &str, new: &str) -> Result<(), String> {
        let new = interface::clean_folder(new);
        circuiter_model::validate_folder(&new)?;
        if new.is_empty() || old.is_empty() {
            return Err("give the folder a name".into());
        }
        if circuiter_model::library::in_folder(&new, old) {
            return Err(format!("{old}/ can't go inside itself"));
        }
        let library = self.folder_library(lib)?;
        if library.all_folders().contains(&new) {
            return Err(format!("{new}/ already exists"));
        }
        clash(library, &new)?;
        let moved = |f: &str| -> Option<String> {
            circuiter_model::library::in_folder(f, old).then(|| format!("{new}{}", &f[old.len()..]))
        };
        let names: Vec<String> = library
            .cells
            .values()
            .filter(|d| moved(&d.folder).is_some())
            .map(|d| d.name.clone())
            .collect();
        let kept: Vec<String> = library.folders.iter().cloned().collect();
        library.folders = kept.iter().map(|f| moved(f).unwrap_or_else(|| f.clone())).collect();
        library.folders.insert(new.clone());
        for name in names {
            let def = library.cells.get_mut(&name).expect("listed");
            def.folder = moved(&def.folder).expect("inside");
        }
        self.dirty = true;
        self.recheck();
        Ok(())
    }

    /// Deletes an empty folder (with any empty folders in it).
    pub fn delete_folder(&mut self, lib: &str, folder: &str) -> Result<(), String> {
        let library = self.folder_library(lib)?;
        if library.folder_has_cells(folder) {
            return Err(format!("{folder}/ isn't empty: move its cells out first"));
        }
        library.folders.retain(|f| !circuiter_model::library::in_folder(f, folder));
        self.dirty = true;
        Ok(())
    }

    /// Moves a cell to `folder` (empty for the top level). The folder it leaves stays.
    /// Undoable while the cell is open.
    pub fn move_cell(&mut self, id: &CellId, folder: &str) -> Result<(), String> {
        let folder = interface::clean_folder(folder);
        circuiter_model::validate_folder(&folder)?;
        let library = self.folder_library(&id.library)?;
        if library.cell(&id.cell).is_some_and(|d| d.folder == folder) {
            return Ok(());
        }
        let path =
            if folder.is_empty() { id.cell.clone() } else { format!("{folder}/{}", id.cell) };
        if library.all_folders().contains(&path) {
            return Err(format!("{folder}/ already has a folder named {}", id.cell));
        }
        let def = library.cells.get_mut(&id.cell).ok_or_else(|| format!("no cell {id}"))?;
        let before = def.clone();
        def.folder = folder;
        if !before.folder.is_empty() {
            library.folders.insert(before.folder.clone());
        }
        self.record(id, before);
        self.dirty = true;
        self.recheck();
        Ok(())
    }

    /// Makes a new component, in the project, from the selected parts and the wires
    /// between them, with a port for each connection they have to the rest of the
    /// sheet, and opens it. The sheet itself is left as it is. Returns the new cell and
    /// its ports.
    pub fn component_from_selection(
        &mut self,
        name: &str,
        folder: &str,
    ) -> Result<(CellId, Vec<String>), String> {
        if self.mode != Mode::Schematic {
            return Err("select parts on a schematic first".into());
        }
        validate_name(name)?;
        let source = self.current().cell.library.clone();
        let def = self.current_def().ok_or("cell not found")?;
        let (mut built, ports) = edit::component_from(
            &self.design,
            &source,
            def,
            &self.selection,
            &self.scene.connectivity,
        )?;
        let project = self.project.name().to_owned();
        if source != project {
            // Parts named relative to their old library keep pointing there.
            for inst in built.schematic.iter_mut().flat_map(|s| &mut s.instances) {
                if inst.cell.library.is_none() {
                    inst.cell.library = Some(source.clone());
                }
            }
        }
        let count = built.schematic.as_ref().map_or(0, |s| s.instances.len());
        self.new_cell(name, folder)?;
        self.edit(|_, _, d| {
            d.schematic = built.schematic.take();
            d.cell.ports = std::mem::take(&mut built.cell.ports);
            Ok(())
        })?;
        let listed = if ports.is_empty() { "no ports".to_owned() } else { ports.join(", ") };
        self.notice = Some(format!(
            "Made {name} from {count} parts (ports: {listed}). Place it from Components → This project"
        ));
        Ok((CellId::new(&project, name), ports))
    }

    /// Adds a part made from a Verilog module to the project, in the `hdl` folder, or
    /// brings an existing one up to date with its module (keeping a drawn symbol).
    pub fn add_hdl_part(&mut self, def: CellDef) -> Result<CellId, String> {
        let lib = self.project.name().to_owned();
        let id = CellId::new(&lib, &def.name);
        let library = self.design.library_mut(&lib).ok_or("project library missing")?;
        match library.cells.get_mut(&def.name) {
            Some(old) if old.digital.as_ref().is_some_and(|d| d.model == "verilog") => {
                let before = old.clone();
                let mut after = old.clone();
                after.cell.ports = def.cell.ports;
                after.cell.params = def.cell.params;
                after.digital = def.digital;
                builder::sync(&before, &mut after);
                *old = after;
                self.record(&id, before);
            }
            Some(_) => return Err(format!("{} already exists and isn't from Verilog", def.name)),
            None => library.insert(CellDef { folder: "hdl".into(), ..def }),
        }
        self.dirty = true;
        self.mark_stale();
        self.recheck();
        self.notice = Some(format!("{id} is a part now: place it from Components → This project"));
        Ok(id)
    }

    /// The operated parts' states in the running circuit, by instance path.
    pub fn live_states(&self) -> BTreeMap<String, String> {
        self.live
            .as_ref()
            .and_then(|l| l.shared.lock().ok().map(|s| s.states.clone()))
            .unwrap_or_default()
    }

    /// Starts the design being viewed running live, at `speed` simulated seconds per
    /// real second.
    pub fn start_live(&mut self, speed: f64) -> Result<(), String> {
        self.stop_live();
        let top = self.sim_top().clone();
        let session = crate::live::Session::start(&self.design, &top, &self.project.root, speed)?;
        self.live_watch = simulation::default_probes(&session.netlist);
        session.send(crate::live::Command::Watch(self.live_watch.clone()));
        self.live = Some(session);
        self.live_revision = self.revision;
        self.live_drawn.clear();
        self.refresh();
        Ok(())
    }

    pub fn stop_live(&mut self) {
        if self.live.take().is_some() {
            self.live_drawn.clear();
            self.refresh();
        }
    }

    /// Redraws the sheet when a switch in the running circuit changed.
    pub fn follow_live(&mut self) {
        if self.live.is_none() {
            return;
        }
        let states = self.live_states();
        if states != self.live_drawn {
            self.live_drawn = states;
            if let Some(scene) = self.build_scene() {
                self.scene = scene;
            }
        }
    }

    /// Shows or hides local net `net` in the live waveforms.
    pub fn toggle_live_watch(&mut self, net: usize) {
        let Some(flat) = self.live_nets().get(net).copied().flatten() else { return };
        match self.live_watch.iter().position(|&n| n == flat) {
            Some(k) => {
                self.live_watch.remove(k);
            }
            None => self.live_watch.push(flat),
        }
        if let Some(live) = &self.live {
            live.send(crate::live::Command::Watch(self.live_watch.clone()));
        }
    }

    /// The instance path, from the run's top, of instance `index` on this sheet.
    pub fn instance_path(&self, index: usize) -> Option<String> {
        let inst = self.schematic()?.instances.get(index)?;
        let sheet = self.sheet_path();
        Some(if sheet.is_empty() { inst.name.clone() } else { format!("{sheet}.{}", inst.name) })
    }

    /// The flat net of each net on the sheet being viewed, in the running circuit.
    pub fn live_nets(&self) -> Vec<Option<usize>> {
        let Some(live) = &self.live else { return Vec::new() };
        let sheet = self.sheet_path();
        let flat = live.netlist.sheets.get(&sheet);
        (0..self.scene.connectivity.nets.len())
            .map(|k| flat.and_then(|f| f.get(k).copied()))
            .collect()
    }

    /// What the active tool does, for the status bar.
    pub fn hint(&self) -> &'static str {
        match (&self.tool, self.mode) {
            (Tool::Select, Mode::Symbol) => {
                "Click to select · drag to move (pins stay on the 100-mil grid) · drag empty space to box-select"
            }
            (tool, _) => tool.hint(),
        }
    }

    /// Adds cells for model-library definitions to the project library. Returns, for
    /// each, the new cell's name and why it can't be simulated yet (if it can't), or
    /// why it couldn't be imported.
    pub fn import_models(
        &mut self,
        defs: &[circuiter_sim::parse::Definition],
    ) -> Vec<ImportOutcome> {
        let lib = self.project.name().to_owned();
        let mut out = Vec::new();
        for d in defs {
            let taken: Vec<String> = self
                .design
                .library(&lib)
                .map(|l| l.cells.keys().cloned().collect())
                .unwrap_or_default();
            match crate::import::import(&self.design, d, &taken) {
                Ok(imported) => {
                    let name = imported.def.name.clone();
                    if let Some(l) = self.design.library_mut(&lib) {
                        l.insert(imported.def);
                    }
                    out.push(Ok((name, imported.warning)));
                }
                Err(e) => out.push(Err(format!("{}: {e}", crate::import::definition_name(d)))),
            }
        }
        if out.iter().any(Result::is_ok) {
            self.dirty = true;
            self.recheck();
        }
        out
    }

    /// Starts placing the pin of `port`, or the first port without a pin.
    pub fn start_pin(&mut self, port: Option<&str>) -> Result<(), String> {
        let def = self.current_def().ok_or("no cell")?;
        let unpinned = symbol_edit::unpinned_ports(def);
        let port = match port {
            Some(p) => p.to_owned(),
            None => unpinned.first().cloned().ok_or("every port already has a pin")?,
        };
        if !unpinned.contains(&port) {
            return Err(format!("`{port}` already has a pin"));
        }
        // Face the body from the side the generator would choose.
        let sides = builder::sides(&def.cell);
        let i = def.cell.ports.iter().position(|p| p.name == port).unwrap_or(0);
        let direction = sides.get(i).map_or(PinDirection::Right, |s| s.direction());
        self.set_mode(Mode::Symbol);
        self.tool = Tool::Pin { port, direction };
        self.selection.clear();
        Ok(())
    }

    /// Whether `id` can be placed on the current sheet: it needs a symbol, the sheet must
    /// be editable, and placing it must not make the design contain itself.
    pub fn placeable(&self, id: &CellId) -> bool {
        self.editable()
            && self.mode == Mode::Schematic
            && self.design.cell(id).is_some_and(|d| d.symbol.is_some())
            && !self.design.contains(id, &self.current().cell)
    }

    /// The reference to use for `id` from the current sheet.
    pub fn cell_ref(&self, id: &CellId) -> CellRef {
        if id.library == self.current().cell.library {
            CellRef::new(None, &id.cell)
        } else {
            CellRef::new(Some(&id.library), &id.cell)
        }
    }

    pub fn start_placing(&mut self, id: &CellId) {
        if self.placeable(id) {
            self.tool =
                Tool::Place { cell: self.cell_ref(id), rotation: Rotation::R0, mirror: false };
            self.selection.clear();
        }
    }

    /// Escape: back out of whatever is in progress, one step at a time.
    pub fn escape(&mut self) {
        if self.wire_from_pin {
            self.wire_from_pin = false;
            self.drag = None;
            self.tool = Tool::Select;
            return;
        }
        match &mut self.tool {
            Tool::Wire { points, .. } if !points.is_empty() => points.clear(),
            Tool::Line { points } if !points.is_empty() => points.clear(),
            Tool::Rect { from: from @ Some(_) } => *from = None,
            Tool::Circle { center: center @ Some(_) } => *center = None,
            Tool::Arc { center: center @ Some(_), start } => {
                *center = None;
                *start = None;
            }
            Tool::Select => {
                if let Some(Drag::Move { base, .. }) = self.drag.take() {
                    let id = self.current().cell.clone();
                    *self.design.cell_mut(&id).expect("exists") = *base;
                    self.refresh();
                }
                self.select_only(None);
            }
            _ => self.tool = Tool::Select,
        }
    }

    pub fn delete_selection(&mut self) {
        let targets = std::mem::take(&mut self.selection);
        let mode = self.mode;
        let _ = self.edit(|_, _, def| {
            match mode {
                Mode::Schematic => edit::delete(def, &targets),
                Mode::Symbol => symbol_edit::delete(def, &targets),
            }
            Ok(())
        });
        self.net = None;
    }

    fn selected_instances(&self) -> Vec<usize> {
        self.selection
            .iter()
            .filter_map(|o| match o {
                Owner::Instance(i) => Some(*i),
                _ => None,
            })
            .collect()
    }

    /// R: rotates the part or pin being placed, or the selection.
    pub fn rotate(&mut self) {
        if let Tool::Pin { direction, .. } = &mut self.tool {
            *direction = rotate_direction(*direction);
            return;
        }
        if self.mode == Mode::Symbol {
            let targets = self.selection.clone();
            let _ = self.edit(|_, _, def| {
                symbol_edit::rotate(def, &targets);
                Ok(())
            });
            return;
        }
        if let Tool::Place { rotation, .. } = &mut self.tool {
            *rotation = match rotation {
                Rotation::R0 => Rotation::R90,
                Rotation::R90 => Rotation::R180,
                Rotation::R180 => Rotation::R270,
                Rotation::R270 => Rotation::R0,
            };
            return;
        }
        let instances = self.selected_instances();
        if !instances.is_empty() {
            let _ = self.edit(|d, lib, def| {
                edit::rotate(d, lib, def, &instances);
                Ok(())
            });
        }
    }

    /// X: mirrors the part or pin being placed, or the selection.
    pub fn mirror(&mut self) {
        if let Tool::Pin { direction, .. } = &mut self.tool {
            *direction = match direction {
                PinDirection::Left => PinDirection::Right,
                PinDirection::Right => PinDirection::Left,
                PinDirection::Up => PinDirection::Down,
                PinDirection::Down => PinDirection::Up,
            };
            return;
        }
        if self.mode == Mode::Symbol {
            let targets = self.selection.clone();
            let _ = self.edit(|_, _, def| {
                symbol_edit::mirror(def, &targets);
                Ok(())
            });
            return;
        }
        if let Tool::Place { mirror, .. } = &mut self.tool {
            *mirror = !*mirror;
            return;
        }
        let instances = self.selected_instances();
        if !instances.is_empty() {
            let _ = self.edit(|d, lib, def| {
                edit::mirror(d, lib, def, &instances);
                Ok(())
            });
        }
    }

    pub fn flip_wire_corner(&mut self) {
        if let Tool::Wire { horizontal_first, .. } = &mut self.tool {
            *horizontal_first = !*horizontal_first;
        }
    }

    /// Whether a wire ending at `p` would connect to something.
    fn is_connection_point(&self, p: Point) -> bool {
        let Some(s) = self.schematic() else { return false };
        let on_segment = |a: Point, b: Point| {
            let cross = (b.x.nm() - a.x.nm()) as i128 * (p.y.nm() - a.y.nm()) as i128
                - (b.y.nm() - a.y.nm()) as i128 * (p.x.nm() - a.x.nm()) as i128;
            cross == 0
                && p.x >= a.x.min(b.x)
                && p.x <= a.x.max(b.x)
                && p.y >= a.y.min(b.y)
                && p.y <= a.y.max(b.y)
        };
        self.scene.connectivity.pins.iter().any(|pin| pin.at == p)
            || s.wires.iter().any(|w| w.points.windows(2).any(|seg| on_segment(seg[0], seg[1])))
            || s.labels.iter().any(|l| l.at == p)
            || s.ports.iter().any(|q| q.at == p)
    }

    fn finish_line(&mut self) {
        if let Tool::Line { points } = &mut self.tool {
            let points = std::mem::take(points);
            if points.len() >= 2 {
                self.add_graphic(Graphic::Polyline { points, fill: false });
            }
        }
    }

    fn add_graphic(&mut self, graphic: Graphic) {
        let _ = self.edit(|_, _, def| Ok(symbol_edit::add_graphic(def, graphic)));
    }

    fn finish_wire(&mut self) {
        if let Tool::Wire { points, .. } = &mut self.tool {
            let points = std::mem::take(points);
            if points.len() >= 2 {
                let _ = self.edit(|_, _, def| {
                    edit::add_wire(def, points);
                    Ok(())
                });
            }
        }
        if std::mem::take(&mut self.wire_from_pin) {
            self.tool = Tool::Select;
        }
    }

    /// Copies the selection (on a sheet) and returns it as text for the system
    /// clipboard. `None` if nothing on a sheet is selected.
    pub fn copy(&mut self) -> Option<String> {
        let (clip, _) = self.copy_selection()?;
        let text = clip.to_text();
        self.clipboard = Some(clip);
        Some(text)
    }

    fn copy_selection(&self) -> Option<(edit::Clip, Point)> {
        let def = self.current_def().filter(|_| self.mode == Mode::Schematic)?;
        let lib = &self.current().cell.library;
        let (clip, centre) = edit::copy(&self.design, lib, def, &self.selection);
        (!clip.is_empty()).then_some((clip, centre))
    }

    /// Copies the selection, then deletes it.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        if self.editable() {
            self.delete_selection();
        }
        Some(text)
    }

    /// Pastes a clip centred on `at` (or the middle of the view) and selects it, ready to
    /// drag into place. `text` is the system clipboard's: a clip copied anywhere, even
    /// in another window; anything else pastes what was last copied here.
    pub fn paste(&mut self, text: Option<&str>, at: Option<Pt>) -> Result<(), String> {
        if self.mode != Mode::Schematic {
            return Err("paste parts onto a schematic".into());
        }
        let clip = text
            .and_then(edit::Clip::from_text)
            .or_else(|| self.clipboard.clone())
            .ok_or("nothing copied yet: select parts and press Ctrl+C")?;
        let at = at
            .or_else(|| self.current().camera.map(|c| c.center))
            .map_or(Point::new(Length(0), Length(0)), to_point);
        self.paste_clip(&clip, at)
    }

    fn paste_clip(&mut self, clip: &edit::Clip, at: Point) -> Result<(), String> {
        let added = self.edit(|d, lib, def| Ok(edit::paste(d, lib, def, clip, at)))?;
        self.tool = Tool::Select;
        self.selection = added;
        self.update_net();
        Ok(())
    }

    /// Ctrl+D: a copy of the selection just below and to the right of it, selected.
    pub fn duplicate(&mut self) -> Result<(), String> {
        let (clip, centre) = self.copy_selection().ok_or("select something to duplicate")?;
        let step = Length(GRID.nm() * 2);
        self.paste_clip(&clip, centre + Point::new(step, step))
    }

    /// The end of a part's pin within `tolerance` of `at`, where a wire can start: only
    /// on an editable schematic.
    pub fn pin_end_at(&self, at: Pt, tolerance: f64) -> Option<Point> {
        if self.mode != Mode::Schematic || !self.editable() {
            return None;
        }
        self.scene
            .connectivity
            .pins
            .iter()
            .map(|pin| {
                let q = Pt::from(pin.at);
                ((q.x - at.x).hypot(q.y - at.y), pin.at)
            })
            .filter(|(d, _)| *d <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }

    /// Starts a wire at pin end `p` from Select; it returns to Select when done.
    fn wire_from(&mut self, p: Point) {
        self.selection.clear();
        self.net = None;
        self.tool = Tool::Wire { points: vec![p], horizontal_first: true };
        self.wire_from_pin = true;
    }

    /// Handles a pointer gesture from the canvas. `tolerance` is the pick distance in mm.
    /// Returns an error message to show, if any.
    pub fn gesture(&mut self, gesture: Gesture, tolerance: f64) -> Option<String> {
        // A new action replaces the last edit's notice.
        if matches!(gesture, Gesture::Click { .. } | Gesture::DragStart { .. }) {
            self.notice = None;
        }
        // Dragging something on the sheet moves it, whatever tool is in hand: a drag
        // never silently does nothing.
        if let Gesture::DragStart { from } = gesture {
            if self.tool != Tool::Select
                && self.editable()
                && self.scene.hit_test(from, tolerance).is_some_and(|o| o != Owner::Derived)
            {
                self.tool = Tool::Select;
                self.wire_from_pin = false;
            }
        }
        let hit = |ws: &Workspace, at: Pt| {
            ws.scene.hit_test(at, tolerance).filter(|o| *o != Owner::Derived)
        };
        // A pin's end is where wires start: click it and click onwards, or drag a wire
        // out of it, without picking the Wire tool first.
        if self.tool == Tool::Select {
            match gesture {
                Gesture::Click { at, shift: false } => {
                    if let Some(p) = self.pin_end_at(at, tolerance) {
                        self.wire_from(p);
                        return None;
                    }
                }
                Gesture::DragStart { from } => {
                    if let Some(p) = self.pin_end_at(from, tolerance) {
                        self.wire_from(p);
                        self.drag = Some(Drag::Wire);
                        return None;
                    }
                }
                _ => {}
            }
        }
        if let (Some(Drag::Wire), Gesture::DragEnd { to, .. }) = (&self.drag, gesture) {
            self.drag = None;
            // Let go on a pin or wire and it connects there; anywhere else it ends there.
            self.gesture(Gesture::Click { at: to, shift: false }, tolerance);
            if matches!(&self.tool, Tool::Wire { points, .. } if !points.is_empty()) {
                self.finish_wire();
            }
            return None;
        }
        if matches!(self.drag, Some(Drag::Wire)) {
            return None;
        }
        match (self.tool.clone(), gesture) {
            (Tool::Select, Gesture::Click { at, shift }) => {
                let h = hit(self, at);
                if shift {
                    if let Some(o) = h {
                        if !self.selection.remove(&o) {
                            self.selection.insert(o);
                        }
                        self.update_net();
                    }
                } else {
                    self.select_only(h);
                }
            }
            (Tool::Select, Gesture::DoubleClick { at }) => {
                if let Some(Owner::Instance(i)) = hit(self, at) {
                    if self.scene.descendable.get(i) == Some(&true) {
                        self.descend(i);
                    }
                }
            }
            (Tool::Select, Gesture::DragStart { from }) => match hit(self, from) {
                Some(o) if self.editable() => {
                    if !self.selection.contains(&o) {
                        self.select_only(Some(o));
                    }
                    let base = Box::new(self.current_def()?.clone());
                    let targets = self.selection.clone();
                    // Pins must stay on the pin grid; drawings may use the finer one.
                    let fine = self.mode == Mode::Symbol
                        && !targets.iter().any(|t| matches!(t, Owner::Pin(_)));
                    let grid = if fine { FINE_GRID } else { GRID };
                    let grab = to_point(from);
                    let from = edit::snap(grab, grid);
                    self.drag = Some(Drag::Move { base, from, grab, grid, targets });
                }
                _ => self.drag = Some(Drag::Band { from }),
            },
            (Tool::Select, Gesture::Drag { to, .. }) => {
                if let Some(Drag::Move { base, from, grab, grid, targets }) = &self.drag {
                    let delta = edit::snap(to_point(to), *grid) - *from;
                    let mut def = (**base).clone();
                    let id = self.current().cell.clone();
                    let lone_wire = match targets.iter().collect::<Vec<_>>()[..] {
                        [Owner::Wire(w)] => Some(*w),
                        _ => None,
                    };
                    match (self.mode, lone_wire) {
                        // One wire: drag the segment under the pointer, as EDA tools do.
                        (Mode::Schematic, Some(w)) => {
                            edit::drag_segment(&self.design, &id.library, &mut def, w, *grab, delta)
                        }
                        (Mode::Schematic, None) => {
                            edit::translate(&self.design, &id.library, &mut def, targets, delta)
                        }
                        (Mode::Symbol, _) => symbol_edit::translate(&mut def, targets, delta),
                    }
                    builder::sync(base, &mut def);
                    *self.design.cell_mut(&id).expect("exists") = def;
                    self.refresh();
                }
            }
            (Tool::Select, Gesture::DragEnd { to, shift, .. }) => match self.drag.take() {
                Some(Drag::Move { base, .. }) => {
                    let id = self.current().cell.clone();
                    if self.design.cell(&id) != Some(&*base) {
                        let before = self.pin_groups(&base);
                        self.record(&id, *base);
                        self.dirty = true;
                        self.note_changes(&before);
                    }
                }
                Some(Drag::Band { from }) => {
                    let mut rect = Rect::EMPTY;
                    rect.include(from);
                    rect.include(to);
                    let found = self.scene.objects_in(&rect);
                    if !shift {
                        self.selection.clear();
                    }
                    self.selection.extend(found);
                    self.update_net();
                }
                Some(Drag::Wire) | None => {}
            },
            (Tool::Place { cell, rotation, mirror }, Gesture::Click { at, shift }) => {
                let placement = Transform { position: snapped(at), rotation, mirror };
                let placed = self.edit(|d, lib, def| Ok(edit::place(d, lib, def, cell, placement)));
                // One part per click, then back to selecting (and moving) things;
                // Shift+click keeps placing copies.
                if let Ok(i) = placed {
                    if !shift {
                        self.tool = Tool::Select;
                        self.select_only(Some(Owner::Instance(i)));
                    }
                }
                return placed.err();
            }
            (Tool::Wire { .. }, Gesture::Click { at, .. }) => {
                let p = snapped(at);
                let connects = self.is_connection_point(p);
                let Tool::Wire { points, horizontal_first } = &mut self.tool else {
                    unreachable!()
                };
                match points.last().copied() {
                    None => points.push(p),
                    Some(last) => {
                        points.extend(edit::route(last, p, *horizontal_first).into_iter().skip(1));
                        if connects && points.first() != Some(&p) {
                            self.finish_wire();
                        }
                    }
                }
            }
            (Tool::Wire { .. }, Gesture::DoubleClick { .. }) => self.finish_wire(),
            (Tool::Label { net }, Gesture::Click { at, .. }) => {
                return self.edit(|_, _, def| edit::add_label(def, &net, snapped(at))).err();
            }
            (Tool::Port { name, kind }, Gesture::Click { at, .. }) => {
                let result = self.edit(|_, _, def| edit::add_port(def, &name, kind, snapped(at)));
                self.tool = Tool::Select;
                return result.err();
            }
            (Tool::Line { .. }, Gesture::Click { at, .. }) => {
                let p = snapped_fine(at);
                let Tool::Line { points } = &mut self.tool else { unreachable!() };
                if points.len() >= 2 && points.first() == Some(&p) {
                    points.push(p);
                    self.finish_line();
                } else if points.last() != Some(&p) {
                    points.push(p);
                }
            }
            (Tool::Line { .. }, Gesture::DoubleClick { .. }) => self.finish_line(),
            (Tool::Rect { from }, Gesture::Click { at, .. }) => {
                let p = snapped_fine(at);
                match from {
                    None => self.tool = Tool::Rect { from: Some(p) },
                    Some(a) if a.x != p.x && a.y != p.y => {
                        self.tool = Tool::Rect { from: None };
                        let (min, max) = (
                            Point::new(a.x.min(p.x), a.y.min(p.y)),
                            Point::new(a.x.max(p.x), a.y.max(p.y)),
                        );
                        self.add_graphic(Graphic::Rect { a: min, b: max, fill: false });
                    }
                    Some(_) => {}
                }
            }
            (Tool::Circle { center }, Gesture::Click { at, .. }) => {
                let p = snapped_fine(at);
                match center {
                    None => self.tool = Tool::Circle { center: Some(p) },
                    Some(c) if c != p => {
                        self.tool = Tool::Circle { center: None };
                        let radius = symbol_edit::distance(c, p);
                        self.add_graphic(Graphic::Circle { center: c, radius, fill: false });
                    }
                    Some(_) => {}
                }
            }
            (Tool::Arc { center, start }, Gesture::Click { at, .. }) => {
                let p = snapped_fine(at);
                match (center, start) {
                    (None, _) => self.tool = Tool::Arc { center: Some(p), start: None },
                    (Some(c), None) if c != p => {
                        self.tool = Tool::Arc { center: Some(c), start: Some(p) }
                    }
                    (Some(c), Some(s)) if c != p => {
                        self.tool = Tool::Arc { center: None, start: None };
                        self.add_graphic(Graphic::Arc {
                            center: c,
                            radius: symbol_edit::distance(c, s),
                            start: symbol_edit::angle(c, s),
                            end: symbol_edit::angle(c, p),
                        });
                    }
                    _ => {}
                }
            }
            (Tool::Text { text }, Gesture::Click { at, .. }) => {
                self.tool = Tool::Select;
                self.add_graphic(Graphic::Text {
                    at: snapped_fine(at),
                    text,
                    size: Length::from_mils(50),
                    anchor: Anchor::Left,
                });
            }
            (Tool::Pin { port, direction }, Gesture::Click { at, .. }) => {
                let result =
                    self.edit(|_, _, def| symbol_edit::add_pin(def, &port, snapped(at), direction));
                if result.is_ok() {
                    // Carry on with the next port that still needs a pin.
                    if self.start_pin(None).is_err() {
                        self.tool = Tool::Select;
                    }
                }
                return result.err();
            }
            _ => {}
        }
        None
    }

    /// What to draw over the sheet for the current tool, given the cursor position.
    /// Operating-point voltages to tag nets with on the sheet being viewed.
    fn voltage_notes(&self) -> Vec<(Pt, String)> {
        if self.live.is_some() {
            return self.live_notes();
        }
        let Some(sim) = self.fresh_sim() else { return Vec::new() };
        if !matches!(
            sim.results,
            Some(Results::Op(_) | Results::Tran { .. } | Results::Logic(_) | Results::Mixed { .. })
        ) {
            return Vec::new();
        }
        // After a transient run, each net shows where it ended up; digital nets show
        // their logic value.
        let value = |flat: usize| -> Option<String> {
            match &sim.results {
                Some(Results::Logic(run)) => run.last(flat).map(|v| v.hex()),
                // Analog nets show volts; nets only digital parts are on show values.
                Some(Results::Mixed { logic, .. }) => match sim.waveform(flat) {
                    Some((_, v)) => v.last().copied().map(volts),
                    None => logic.last(flat).map(|v| v.hex()),
                },
                Some(Results::Tran { .. }) => {
                    sim.waveform(flat).and_then(|(_, v)| v.last().copied()).map(volts)
                }
                _ => sim.voltage(flat).map(volts),
            }
        };
        let mut out = self.net_tags(&|k| self.flat_net(k).and_then(value));
        let readings: BTreeMap<String, crate::live::Reading> =
            crate::live::Meter::all(&sim.netlist)
                .iter()
                .filter_map(|m| Some((m.path.clone(), sim.meter(m)?)))
                .collect();
        out.extend(self.meter_tags(&readings));
        out
    }

    /// Values on the wires of the running circuit, and its meters' readings.
    fn live_notes(&self) -> Vec<(Pt, String)> {
        let Some(live) = &self.live else { return Vec::new() };
        let Ok(shared) = live.shared.lock() else { return Vec::new() };
        let nets = self.live_nets();
        let mut out = self.net_tags(&|k| {
            let f = nets.get(k).copied().flatten()?;
            shared
                .voltages
                .get(f)
                .copied()
                .flatten()
                .map(volts)
                .or_else(|| shared.logic.get(f).copied().flatten().map(|v| v.hex()))
        });
        out.extend(self.meter_tags(&shared.readings));
        out
    }

    /// Multimeters' readings, over their bodies.
    fn meter_tags(&self, readings: &BTreeMap<String, crate::live::Reading>) -> Vec<(Pt, String)> {
        let Some(schematic) = self.schematic() else { return Vec::new() };
        let sheet = self.sheet_path();
        let mut out = Vec::new();
        for (i, inst) in schematic.instances.iter().enumerate() {
            let path =
                if sheet.is_empty() { inst.name.clone() } else { format!("{sheet}.{}", inst.name) };
            let (Some(r), Some(body)) = (readings.get(&path), self.scene.instance_bodies.get(i))
            else {
                continue;
            };
            out.push((Pt::new(body.center().x - 4.0, body.min.y + 3.0), r.show()));
        }
        out
    }

    /// A tag for each net on the sheet with a value: in the middle of its longest
    /// wire, else at a label or pin.
    fn net_tags(&self, value_of: &dyn Fn(usize) -> Option<String>) -> Vec<(Pt, String)> {
        let Some(schematic) = self.schematic() else { return Vec::new() };
        let conn = &self.scene.connectivity;
        let mut out = Vec::new();
        for (k, net) in conn.nets.iter().enumerate() {
            let Some(v) = value_of(k) else { continue };
            let segment = net
                .items
                .iter()
                .filter_map(|it| match it {
                    Item::Wire(w) => schematic.wires.get(*w),
                    _ => None,
                })
                .flat_map(|w| w.points.windows(2).map(|s| (s[0], s[1])))
                .max_by_key(|(a, b)| (a.x - b.x).nm().abs() + (a.y - b.y).nm().abs());
            let at = match segment {
                Some((a, b)) => Pt::new((a.x.mm() + b.x.mm()) / 2.0, (a.y.mm() + b.y.mm()) / 2.0),
                None => match net.items.first() {
                    Some(Item::Pin(p)) => Pt::from(conn.pins[*p].at),
                    Some(Item::Label(l)) => Pt::from(schematic.labels[*l].at),
                    Some(Item::Port(p)) => Pt::from(schematic.ports[*p].at),
                    _ => continue,
                },
            };
            out.push((at, v));
        }
        out
    }

    /// Heat glows and damage marks for the parts on the sheet being viewed.
    fn physics_marks(&self) -> (Vec<Heat>, Vec<(Pt, String)>) {
        let (Some(sim), Some(schematic)) = (self.fresh_sim(), self.schematic()) else {
            return (Vec::new(), Vec::new());
        };
        let base = self.sheet_path();
        let path =
            |name: &str| if base.is_empty() { name.to_owned() } else { format!("{base}.{name}") };
        let mut heat = Vec::new();
        let mut damage = Vec::new();
        for (i, inst) in schematic.instances.iter().enumerate() {
            let Some(bounds) = self.scene.instance_bounds.get(i).copied() else { continue };
            let p = path(&inst.name);
            let inside = format!("{p}.");
            // The hottest part at or inside this instance, against its own limit.
            let mut level: Option<f32> = None;
            if let Some(m) = &sim.monitor {
                for part in m.parts.iter().filter(|q| q.path == p || q.path.starts_with(&inside)) {
                    if let Some(max) = part.max_temperature() {
                        let l = ((part.peak - m.ambient) / (max - m.ambient)) as f32;
                        level = Some(level.map_or(l, |x: f32| x.max(l)));
                    }
                }
            }
            // An operating point that would destroy it glows at full heat.
            if sim.assessments.iter().any(|a| a.path == p || a.path.starts_with(&inside)) {
                level = Some(1.0);
            }
            if let Some(l) = level.filter(|l| *l > 0.05) {
                heat.push((bounds, l));
            }
            let events: Vec<_> = sim
                .events()
                .iter()
                .filter(|e| e.path == p || e.path.starts_with(&inside))
                .collect();
            if let Some(e) = events.first() {
                let what = if e.path == p {
                    crate::simulation::effect_word(e).to_owned()
                } else {
                    format!("{} destroyed inside", e.path.trim_start_matches(&inside))
                };
                damage.push((Pt::new(bounds.min.x, bounds.min.y), what));
            }
        }
        (heat, damage)
    }

    pub fn overlay(&self, cursor: Option<Pt>) -> Overlay {
        let (heat, damage) = self.physics_marks();
        let mut overlay =
            Overlay { notes: self.voltage_notes(), heat, damage, ..Overlay::default() };
        let lib = &self.current().cell.library;
        if let (Some(Drag::Band { from }), Some(c)) = (&self.drag, cursor) {
            overlay.band = Some((*from, c));
        }
        let Some(cursor) = cursor else { return overlay };
        let fine = self.tool.for_symbols() && !matches!(self.tool, Tool::Pin { .. });
        let at = if fine { snapped_fine(cursor) } else { snapped(cursor) };
        if self.tool != Tool::Select {
            overlay.snap = Some(at.into());
        } else if self.drag.is_none() {
            // Over a pin's end: mark it, as that's where a click or drag starts a wire.
            overlay.snap = self.pin_end_at(cursor, self.pick_tolerance).map(Pt::from);
        }
        if self.tool.for_symbols() {
            overlay.ghost = self.symbol_ghost(at);
            return overlay;
        }
        let text = |text: String, role| Element {
            role,
            owner: Owner::Derived,
            shape: Shape::Text {
                at: Pt::new(at.x.mm() + 0.254, at.y.mm() - 0.254),
                text,
                size: 1.27,
                halign: HAlign::Left,
                valign: VAlign::Bottom,
                vertical: false,
            },
        };
        match &self.tool {
            Tool::Select => {}
            Tool::Place { cell, rotation, mirror } => {
                let name = self.schematic().map_or("?".into(), |s| {
                    let prefix = self.design.resolve(cell, lib).map(|(_, d)| d.cell.prefix.clone());
                    edit::next_instance_name(s, &prefix.unwrap_or_default())
                });
                let inst = Instance {
                    name,
                    cell: cell.clone(),
                    placement: Transform { position: at, rotation: *rotation, mirror: *mirror },
                    params: BTreeMap::new(),
                    view: None,
                };
                overlay.ghost = instance_elements(&self.design, lib, &inst);
            }
            Tool::Wire { points, horizontal_first } => {
                if let Some(&last) = points.last() {
                    let mut all = points.clone();
                    all.extend(edit::route(last, at, *horizontal_first).into_iter().skip(1));
                    overlay.wire = all.into_iter().map(Pt::from).collect();
                }
            }
            Tool::Label { net } => overlay.ghost.push(text(net.clone(), Role::NetLabel)),
            Tool::Port { name, .. } => overlay.ghost.push(text(format!("port {name}"), Role::Port)),
            _ => {}
        }
        overlay
    }

    /// What a symbol tool would add with the cursor at `at`.
    fn symbol_ghost(&self, at: Point) -> Vec<Element> {
        let graphic = match &self.tool {
            Tool::Line { points } if !points.is_empty() => {
                let mut points = points.clone();
                points.push(at);
                Graphic::Polyline { points, fill: false }
            }
            Tool::Rect { from: Some(a) } => Graphic::Rect { a: *a, b: at, fill: false },
            Tool::Circle { center: Some(c) } => {
                Graphic::Circle { center: *c, radius: symbol_edit::distance(*c, at), fill: false }
            }
            Tool::Arc { center: Some(c), start: None } => {
                Graphic::Polyline { points: vec![*c, at], fill: false }
            }
            Tool::Arc { center: Some(c), start: Some(s) } => Graphic::Arc {
                center: *c,
                radius: symbol_edit::distance(*c, *s),
                start: symbol_edit::angle(*c, *s),
                end: symbol_edit::angle(*c, at),
            },
            Tool::Text { text } => Graphic::Text {
                at,
                text: text.clone(),
                size: Length::from_mils(50),
                anchor: Anchor::Left,
            },
            Tool::Pin { port, direction } => {
                let symbol = Symbol {
                    generated: false,
                    states: Vec::new(),
                    graphics: Vec::new(),
                    pins: vec![circuiter_model::symbol::SymbolPin {
                        port: port.clone(),
                        at: Point::ORIGIN,
                        direction: *direction,
                        length: GRID,
                        show_name: true,
                    }],
                };
                return self.preview(symbol, at);
            }
            _ => return Vec::new(),
        };
        let symbol = Symbol { graphics: vec![graphic], ..Symbol::default() };
        self.preview(symbol, Point::ORIGIN)
    }

    /// Draws a stand-alone symbol at `at`, with this cell's texts substituted.
    fn preview(&self, symbol: Symbol, at: Point) -> Vec<Element> {
        let Some(def) = self.current_def() else { return Vec::new() };
        let mut library = circuiter_model::Library::new("preview");
        library.insert(CellDef { symbol: Some(symbol), ..def.clone() });
        let mut design = Design::new();
        if design.add_library(library).is_err() {
            return Vec::new();
        }
        let inst = Instance {
            name: format!("{}?", def.cell.prefix),
            cell: circuiter_model::CellRef::new(None, &def.name),
            placement: Transform::at(at),
            params: BTreeMap::new(),
            view: None,
        };
        instance_elements(&design, "preview", &inst)
    }

    fn mark_stale(&mut self) {
        self.revision += 1;
        if let Some(sim) = &mut self.sim {
            sim.stale = true;
        }
    }

    /// The cell a simulation runs: the top of the hierarchy being viewed.
    pub fn sim_top(&self) -> &CellId {
        &self.levels[0].cell
    }

    /// Instance path of the sheet being viewed, from the simulated top: `""`, `U1`...
    pub fn sheet_path(&self) -> String {
        let names: Vec<&str> = self.levels[1..].iter().filter_map(|l| l.via.as_deref()).collect();
        names.join(".")
    }

    /// The current results, if they still describe the design on screen.
    pub fn fresh_sim(&self) -> Option<&Simulation> {
        self.sim.as_ref().filter(|s| !s.stale && s.results.is_some() && &s.top == self.sim_top())
    }

    /// The simulated flat net of local net `net` on the sheet being viewed.
    pub fn flat_net(&self, net: usize) -> Option<usize> {
        let sim = self.fresh_sim()?;
        if self.mode != Mode::Schematic {
            return None;
        }
        sim.netlist.sheets.get(&self.sheet_path())?.get(net).copied()
    }

    /// Flattens and simulates the design being viewed, keeping the plotted nets of the
    /// previous run when it was of the same design.
    pub fn simulate(&mut self, analysis: Analysis) {
        let top = self.sim_top().clone();
        let mut sim = match Simulation::prepare_in(&self.design, &top, &self.project.root) {
            Ok(s) => s,
            Err(e) => {
                self.sim = None;
                self.sim_error = Some(e);
                return;
            }
        };
        self.sim_error = None;
        if let Some(old) = &self.sim {
            if old.top == top {
                let names: Vec<&str> =
                    old.probes.iter().map(|&p| old.netlist.nets[p].name.as_str()).collect();
                let kept: Vec<usize> = names.iter().filter_map(|n| sim.netlist.net(n)).collect();
                if !kept.is_empty() {
                    sim.probes = kept;
                }
            }
        }
        if let Some(old) = &mut self.sim {
            old.cancel();
        }
        sim.start(analysis);
        self.sim = Some(sim);
    }

    /// What ▶ Run does: switch the circuit on and watch it for a sensible time.
    pub fn run_analysis(&self) -> Analysis {
        let (tstop, tstep) = self.default_tran();
        Analysis::Tran { tstop, tstep, power_on: true }
    }

    /// A sensible transient run for the design being viewed.
    pub fn default_tran(&self) -> (f64, f64) {
        let p = self.run_plan();
        (p.tstop, p.tstep)
    }

    /// How long Run goes, for hints: "for 1 µs", or "until its program ends (ECALL)".
    pub fn run_span(&self) -> String {
        let p = self.run_plan();
        let most = simulation::show(p.tstop, "s");
        if p.until_halt {
            format!("until its program ends (ECALL), {most} at most")
        } else {
            format!("for {most}")
        }
    }

    /// What Run will do for the design being viewed; worked out once per change.
    pub fn run_plan(&self) -> simulation::RunPlan {
        let top = self.sim_top().clone();
        if let Some((rev, id, plan)) = self.plan_cache.borrow().as_ref() {
            if *rev == self.revision && *id == top {
                return *plan;
            }
        }
        let plan = simulation::plan(&self.design, &top).unwrap_or(simulation::RunPlan {
            tstop: 1e-3,
            tstep: 1e-6,
            until_halt: false,
        });
        *self.plan_cache.borrow_mut() = Some((self.revision, top, plan));
        plan
    }

    /// Adds or removes the net of local net `net` from the waveform plot.
    pub fn toggle_probe(&mut self, net: usize) {
        let Some(flat) = self.flat_net(net) else { return };
        let Some(sim) = &mut self.sim else { return };
        match sim.probes.iter().position(|&p| p == flat) {
            Some(i) => {
                sim.probes.remove(i);
            }
            None => sim.probes.push(flat),
        }
    }

    pub fn problems(&self) -> Vec<(Severity, String)> {
        let id = self.current().cell.to_string();
        let mut out: Vec<(Severity, String)> = self
            .diagnostics
            .iter()
            .filter(|d| d.location == id || d.location.starts_with(&format!("{id}/")))
            .map(|d| (d.severity, d.message.clone()))
            .collect();
        if self.levels.len() == 1 {
            if let Some(e) = &self.sim_error {
                out.push((Severity::Error, format!("simulation: {e}")));
            }
            if let Some(sim) = self.sim.as_ref().filter(|s| !s.stale) {
                out.extend(sim.diagnostics.iter().map(|d| (d.severity, d.message.clone())));
                out.extend(sim.events().iter().map(|e| (Severity::Error, e.describe())));
                for a in &sim.assessments {
                    let when = match a.time_to_failure {
                        Some(0.0) => " at once".to_owned(),
                        Some(t) => {
                            format!(" in about {}", circuiter_model::units::format_quantity(t, "s"))
                        }
                        None => String::new(),
                    };
                    out.push((
                        Severity::Warning,
                        format!("{} would fail{when}: {}", a.path, a.reason),
                    ));
                }
                if let Some(e) = &sim.error {
                    out.push((Severity::Error, format!("simulation: {e}")));
                }
            }
        }
        if let Some(schematic) = self.schematic() {
            let conn = &self.scene.connectivity;
            let lib = &self.current().cell.library;
            for &i in &conn.dangling_pins {
                let pin = &conn.pins[i];
                let inst = &schematic.instances[pin.instance];
                // An instrument's spare probes may hang free.
                let probe = self.design.resolve(&inst.cell, lib).is_some_and(|(_, d)| d.cell.probe);
                if !probe {
                    out.push((
                        Severity::Warning,
                        format!("pin {}.{} is not connected", inst.name, pin.port),
                    ));
                }
            }
            for p in &conn.dangling_wire_ends {
                out.push((Severity::Warning, format!("wire end at {p} is not connected")));
            }
            for &(pin, _) in &conn.passes_over {
                let pin = &conn.pins[pin];
                let inst = &schematic.instances[pin.instance].name;
                out.push((
                    Severity::Warning,
                    format!(
                        "a wire passes over pin {inst}.{} without connecting; end the wire or put a corner there to connect it",
                        pin.port
                    ),
                ));
            }
            for net in &conn.nets {
                if net.names.len() > 1 {
                    out.push((
                        Severity::Warning,
                        format!("net has conflicting names: {}", net.names.join(", ")),
                    ));
                }
            }
        }
        out
    }

    /// A one-line description of an object, for the status bar. Empty for an object
    /// that no longer exists: the pointer may still be over something just deleted.
    pub fn describe(&self, owner: Owner) -> String {
        let conn = &self.scene.connectivity;
        let net_name =
            |item| conn.net_of(item).and_then(|n| conn.nets.get(n)).map(|n| n.name.clone());
        if let (Some(symbol), Some(def)) = (self.symbol(), self.current_def()) {
            return match owner {
                Owner::Pin(i) => symbol.pins.get(i).map(|pin| {
                    let kind = def.cell.port(&pin.port).map(|p| format!("{:?}", p.kind));
                    format!("pin {}  {}", pin.port, kind.unwrap_or_default())
                }),
                Owner::Graphic(i) => symbol.graphics.get(i).map(|g| graphic_name(g).to_owned()),
                _ => None,
            }
            .unwrap_or_default();
        }
        let Some(schematic) = self.schematic() else {
            return self.current().cell.to_string();
        };
        match owner {
            Owner::Instance(i) => schematic.instances.get(i).map(|inst| {
                let hint = if self.scene.descendable.get(i) == Some(&true) {
                    "  (double-click to open)"
                } else {
                    ""
                };
                format!("{}  {}{hint}", inst.name, inst.cell)
            }),
            Owner::Wire(i) => net_name(Item::Wire(i)).map(|n| format!("wire on net {n}")),
            Owner::Label(i) => net_name(Item::Label(i)).map(|n| format!("label on net {n}")),
            Owner::Port(i) => schematic.ports.get(i).map(|p| format!("port {}", p.port)),
            Owner::Graphic(_) | Owner::Pin(_) | Owner::Derived => None,
        }
        .unwrap_or_default()
    }
}

/// On disk a folder and a cell can't share a directory: `folder` mustn't be where a
/// cell lives.
fn clash(library: &circuiter_model::Library, folder: &str) -> Result<(), String> {
    let (parent, leaf) = folder.rsplit_once('/').unwrap_or(("", folder));
    match library.cell(leaf) {
        Some(d) if d.folder == parent => {
            Err(format!("there is a cell named {leaf} there; pick another name"))
        }
        _ => Ok(()),
    }
}

/// A voltage for a tag; below a microvolt is numerical noise, called zero.
fn volts(v: f64) -> String {
    simulation::show(if v.abs() < 1e-6 { 0.0 } else { v }, "V")
}

pub fn graphic_name(g: &Graphic) -> &'static str {
    match g {
        Graphic::Polyline { points, .. } if points.len() > 2 && points.first() == points.last() => {
            "polygon"
        }
        Graphic::Polyline { .. } => "line",
        Graphic::Rect { .. } => "rectangle",
        Graphic::Circle { .. } => "circle",
        Graphic::Arc { .. } => "arc",
        Graphic::Text { .. } => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cmos_inverter")
    }

    fn example() -> Workspace {
        Workspace::open(&example_dir()).unwrap()
    }

    /// A writable copy of the example, for tests that save.
    fn example_copy() -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        copy_dir(&example_dir(), tmp.path());
        let ws = Workspace::open(tmp.path()).unwrap();
        (tmp, ws)
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let dest = to.join(path.file_name().unwrap());
            if path.is_dir() {
                copy_dir(&path, &dest);
            } else {
                std::fs::copy(&path, &dest).unwrap();
            }
        }
    }

    fn instance(ws: &Workspace, name: &str) -> usize {
        ws.schematic().unwrap().instances.iter().position(|i| i.name == name).unwrap()
    }

    fn click(ws: &mut Workspace, x: f64, y: f64) -> Option<String> {
        ws.gesture(Gesture::Click { at: Pt::new(x, y), shift: false }, 0.5)
    }

    /// A fresh, empty sheet in a writable project.
    fn blank_sheet() -> (tempfile::TempDir, Workspace) {
        let (tmp, mut ws) = example_copy();
        ws.new_cell("scratch", "").unwrap();
        (tmp, ws)
    }

    #[test]
    fn cells_open_in_tabs_that_remember_their_view() {
        let (_tmp, mut ws) = example_copy();
        let top = ws.current().cell.clone();
        assert_eq!(ws.tabs(), std::slice::from_ref(&top));
        let camera = Camera { center: Pt::new(1.0, 2.0), scale: 9.0, fitted: None };
        ws.levels.last_mut().unwrap().camera = Some(camera);
        // Into a part on the top sheet, then open a library part: a second tab.
        ws.descend(instance(&ws, "U1"));
        ws.open_cell(CellId::new("std", "nmos"));
        assert_eq!(ws.tabs().len(), 2);
        assert_eq!(ws.active_tab(), 1);
        // The first tab is still inside U1.
        assert_eq!(ws.tabs()[0].cell, "inverter");
        ws.switch_tab(0);
        assert_eq!(ws.levels.len(), 2, "as it was left");
        ws.ascend_to(1);
        assert_eq!(ws.current().camera, Some(camera));
        // Opening a cell that has a tab goes to it; closing the active tab shows the
        // one before it, and the last tab can't be closed.
        ws.open_cell(CellId::new("std", "nmos"));
        assert_eq!(ws.active_tab(), 1);
        ws.close_tab(1);
        assert_eq!(ws.tabs(), std::slice::from_ref(&top));
        assert_eq!(ws.current().cell, top);
        ws.close_tab(0);
        assert_eq!(ws.tabs().len(), 1);
        // Back reopens a closed tab.
        ws.go_back();
        assert_eq!(ws.current().cell.cell, "nmos");
        assert_eq!(ws.tabs().len(), 2);
    }

    #[test]
    fn back_and_forward_return_to_where_you_were() {
        let (_tmp, mut ws) = example_copy();
        let top = ws.current().cell.clone();
        assert_eq!(ws.back_to(), None);
        let camera = Camera { center: Pt::new(3.0, 4.0), scale: 7.0, fitted: None };
        ws.levels.last_mut().unwrap().camera = Some(camera);
        // Look at a library symbol, then go back to the sheet as it was left.
        ws.open_cell(CellId::new("std", "nmos"));
        assert_eq!(ws.mode, Mode::Symbol);
        assert_eq!(ws.back_to(), Some(&top));
        ws.go_back();
        assert_eq!(ws.current().cell, top);
        assert_eq!(ws.mode, Mode::Schematic);
        assert_eq!(ws.current().camera, Some(camera), "same zoom and position");
        assert_eq!(ws.forward_to().map(|c| c.cell.as_str()), Some("nmos"));
        ws.go_forward();
        assert_eq!(ws.current().cell.cell, "nmos");
        // Going into a part and out again are steps too.
        ws.go_back();
        let u1 = instance(&ws, "U1");
        ws.descend(u1);
        ws.go_back();
        assert_eq!(ws.levels.len(), 1);
        assert_eq!(ws.forward_to().map(|c| c.cell.as_str()), Some("inverter"));
    }

    #[test]
    fn cells_live_in_folders_on_disk_and_can_move() {
        let (tmp, mut ws) = example_copy();
        assert!(ws.new_cell("bad", "9lives").is_err());
        ws.new_cell("half_adder", " logic/ arith/").unwrap();
        let lib = ws.project.name().to_owned();
        let folder = |ws: &Workspace| {
            ws.design.cell(&CellId::new(&lib, "half_adder")).unwrap().folder.clone()
        };
        assert_eq!(folder(&ws), "logic/arith");
        ws.save().unwrap();
        let cells = tmp.path().join("cells");
        assert!(cells.join("logic/arith/half_adder/schematic.ron").is_file());

        // Moving is undoable, and saved by moving the files. The folder left behind
        // stays, empty, until deleted.
        let id = CellId::new(&lib, "half_adder");
        ws.move_cell(&id, "logic").unwrap();
        ws.undo();
        assert_eq!(folder(&ws), "logic/arith");
        ws.redo();
        ws.save().unwrap();
        assert!(!cells.join("logic/arith/half_adder").exists());
        assert!(cells.join("logic/arith").is_dir());
        assert!(cells.join("logic/half_adder/cell.ron").is_file());
        let mut reopened = Workspace::open(tmp.path()).unwrap();
        assert_eq!(folder(&reopened), "logic");

        // Folders: make them nested and empty, rename with contents, delete when empty.
        let ws = &mut reopened;
        assert_eq!(ws.new_folder(&lib, "cpu/alu").unwrap(), "cpu/alu");
        assert!(ws.new_folder(&lib, "cpu/alu").is_err(), "exists");
        assert!(ws.new_folder("std", "mine").is_err(), "std is read-only");
        ws.move_cell(&id, "cpu/alu").unwrap();
        assert!(ws.rename_folder(&lib, "cpu", "cpu/inner").is_err(), "not into itself");
        ws.rename_folder(&lib, "cpu", "core").unwrap();
        assert_eq!(folder(ws), "core/alu");
        assert!(ws.delete_folder(&lib, "core").is_err(), "not empty");
        ws.delete_folder(&lib, "logic/arith").unwrap();
        ws.save().unwrap();
        assert!(cells.join("core/alu/half_adder/schematic.ron").is_file());
        assert!(!cells.join("cpu").exists());
        assert!(!cells.join("logic/arith").exists());
        let folders = ws.design.library(&lib).unwrap().all_folders();
        assert_eq!(folders, ["core", "core/alu", "logic"]);
    }

    #[test]
    fn creates_a_project_named_after_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-amp 2");
        std::fs::create_dir(&dir).unwrap();
        let ws = Workspace::create(&dir).unwrap();
        assert_eq!(ws.project.name(), "my_amp_2");
        assert_eq!(ws.current().cell, CellId::new("my_amp_2", "top"));
        assert!(ws.editable() && ws.problems().is_empty(), "{:?}", ws.problems());
        assert!(Workspace::create(&dir).is_err(), "never overwrites a project");
    }

    #[test]
    fn opens_the_top_cell_without_problems() {
        let ws = example();
        assert_eq!(ws.current().cell, CellId::new("cmos_inverter", "testbench"));
        assert_eq!(ws.mode, Mode::Schematic);
        assert!(ws.editable());
        assert!(ws.problems().is_empty(), "{:?}", ws.problems());
    }

    #[test]
    fn descends_into_an_instance_and_back_out() {
        let mut ws = example();
        let u1 = instance(&ws, "U1");
        assert!(ws.scene.descendable[u1]);
        ws.descend(u1);
        assert_eq!(ws.levels.len(), 2);
        assert_eq!(ws.current().cell, CellId::new("cmos_inverter", "inverter"));
        assert_eq!(ws.current().via.as_deref(), Some("U1"));
        ws.ascend_to(1);
        assert_eq!(ws.levels.len(), 1);
        assert_eq!(ws.selection, BTreeSet::from([Owner::Instance(u1)]));
    }

    #[test]
    fn library_parts_are_read_only_symbols() {
        let mut ws = example();
        ws.open_cell(CellId::new("std", "nmos"));
        assert_eq!(ws.mode, Mode::Symbol);
        assert!(!ws.editable());
        assert!(ws.edit(|_, _, _| Ok(())).is_err());
    }

    #[test]
    fn selecting_a_wire_highlights_its_net() {
        let mut ws = example();
        click(&mut ws, 44.0, 0.0); // on the OUT wire
        let net = ws.net.expect("net selected");
        assert_eq!(ws.scene.connectivity.nets[net].name, "OUT");
        click(&mut ws, 100.0, 100.0);
        assert!(ws.selection.is_empty());
        assert_eq!(ws.net, None);
    }

    /// Two resistors side by side on a blank sheet: R1 with pins at x = ±5.08, R2 with
    /// pins at 15.24 and 25.4.
    fn two_resistors() -> (tempfile::TempDir, Workspace) {
        let (tmp, mut ws) = blank_sheet();
        ws.start_placing(&CellId::new("std", "resistor"));
        let place = |ws: &mut Workspace, x| {
            ws.gesture(Gesture::Click { at: Pt::new(x, 0.0), shift: true }, 0.5)
        };
        assert!(place(&mut ws, 0.0).is_none());
        assert!(place(&mut ws, 20.32).is_none());
        ws.escape();
        (tmp, ws)
    }

    #[test]
    fn dragging_out_of_a_pin_draws_a_wire() {
        let (_tmp, mut ws) = two_resistors();
        assert_eq!(ws.tool, Tool::Select);
        assert_eq!(ws.pin_end_at(Pt::new(5.2, 0.1), 0.5), Some(Point::mm(5.08, 0.0)));
        assert_eq!(ws.pin_end_at(Pt::new(0.0, 0.0), 0.5), None, "the body isn't a pin end");
        let from = Pt::new(5.1, 0.0);
        ws.gesture(Gesture::DragStart { from }, 0.5);
        ws.gesture(Gesture::Drag { from, to: Pt::new(10.0, 0.0) }, 0.5);
        let end = Gesture::DragEnd { from, to: Pt::new(15.2, 0.1), shift: false };
        ws.gesture(end, 0.5);
        assert_eq!(ws.tool, Tool::Select, "back to selecting");
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances.len(), 2, "nothing moved or placed");
        assert_eq!(s.wires.len(), 1);
        assert_eq!(s.wires[0].points, vec![Point::mm(5.08, 0.0), Point::mm(15.24, 0.0)]);
        assert_eq!(ws.scene.connectivity.dangling_pins.len(), 2);

        // Let go in empty space: the wire ends there, dangling.
        let from = Pt::new(25.4, 0.0);
        ws.gesture(Gesture::DragStart { from }, 0.5);
        let end = Gesture::DragEnd { from, to: Pt::new(30.48, 5.08), shift: false };
        ws.gesture(end, 0.5);
        let w = &ws.schematic().unwrap().wires[1].points;
        assert_eq!(w.first(), Some(&Point::mm(25.4, 0.0)));
        assert_eq!(w.last(), Some(&Point::mm(30.48, 5.08)));
        assert_eq!(ws.tool, Tool::Select);
    }

    #[test]
    fn clicking_a_pin_starts_a_wire() {
        let (_tmp, mut ws) = two_resistors();
        click(&mut ws, 5.08, 0.0);
        assert_eq!(
            ws.tool,
            Tool::Wire { points: vec![Point::mm(5.08, 0.0)], horizontal_first: true }
        );
        click(&mut ws, 10.16, 5.08); // a corner
        click(&mut ws, 15.24, 0.0); // R2.a: connects and finishes
        assert_eq!(ws.tool, Tool::Select);
        assert_eq!(ws.schematic().unwrap().wires.len(), 1);
        assert_eq!(ws.scene.connectivity.dangling_pins.len(), 2);
        // Escape gives up on a wire started this way, back to Select.
        click(&mut ws, -5.08, 0.0);
        assert!(matches!(ws.tool, Tool::Wire { .. }));
        ws.escape();
        assert_eq!(ws.tool, Tool::Select);
        assert_eq!(ws.schematic().unwrap().wires.len(), 1);
        // Clicking a part's body still selects it.
        click(&mut ws, 0.0, 0.0);
        assert_eq!(ws.selection, BTreeSet::from([Owner::Instance(0)]));
    }

    #[test]
    fn copy_paste_and_duplicate_parts_with_their_wires() {
        let (_tmp, mut ws) = two_resistors();
        // R1.b to R2.a, plus a stub off R2.b.
        ws.edit(|_, _, d| {
            edit::add_wire(d, vec![Point::mm(5.08, 0.0), Point::mm(15.24, 0.0)]);
            edit::add_wire(d, vec![Point::mm(25.4, 0.0), Point::mm(30.48, 0.0)]);
            Ok(())
        })
        .unwrap();
        assert!(ws.copy().is_none(), "nothing selected");
        ws.selection = BTreeSet::from([Owner::Instance(0), Owner::Instance(1)]);
        let text = ws.copy().unwrap();
        assert!(text.starts_with("circuiter clip 1"));

        // Pasted under the cursor, with fresh names and the wire between the two parts
        // (not the stub, which only touches one of them).
        ws.paste(Some(&text), Some(Pt::new(10.2, 20.4))).unwrap();
        let s = ws.schematic().unwrap();
        let names: Vec<&str> = s.instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["R1", "R2", "R3", "R4"]);
        assert_eq!(s.instances[2].placement.position, Point::mm(0.0, 20.32));
        assert_eq!(s.instances[2].cell.to_string(), "std:resistor");
        assert_eq!(s.wires.len(), 3);
        assert_eq!(s.wires[2].points, vec![Point::mm(5.08, 20.32), Point::mm(15.24, 20.32)]);
        let pasted = BTreeSet::from([Owner::Instance(2), Owner::Instance(3), Owner::Wire(2)]);
        assert_eq!(ws.selection, pasted, "selected, ready to drag");
        assert_eq!(ws.scene.connectivity.dangling_pins.len(), 3, "R1.a, R3.a, R4.b");
        ws.undo();
        assert_eq!(ws.schematic().unwrap().instances.len(), 2, "one undo step");

        // Text that isn't a clip pastes what was copied here.
        ws.paste(Some("hello"), Some(Pt::new(0.0, -20.32))).unwrap();
        assert_eq!(ws.schematic().unwrap().instances.len(), 4);
        ws.undo();

        // Duplicate: a copy beside the original, 200 mil down and right.
        ws.selection = BTreeSet::from([Owner::Instance(1)]);
        ws.duplicate().unwrap();
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances[2].name, "R3");
        assert_eq!(s.instances[2].placement.position, Point::mm(25.4, 5.08));

        // Cut takes it away again.
        ws.cut().unwrap();
        assert_eq!(ws.schematic().unwrap().instances.len(), 2);
    }

    #[test]
    fn pasting_into_another_library_names_the_parts_library() {
        let (_tmp, mut ws) = example_copy();
        let lib = ws.project.name().to_owned();
        ws.selection = BTreeSet::from([Owner::Instance(instance(&ws, "U1"))]);
        let text = ws.copy().unwrap();
        // The inverter lives in the project library: pasted on a project sheet the
        // reference stays short.
        ws.new_cell("other", "").unwrap();
        ws.paste(Some(&text), None).unwrap();
        let inst = &ws.schematic().unwrap().instances[0];
        assert_eq!(inst.cell.to_string(), "inverter");
        assert_eq!(
            edit::Clip::from_text(&text).unwrap().instances[0].cell.to_string(),
            format!("{lib}:inverter")
        );
    }

    #[test]
    fn place_wire_and_connect_two_resistors() {
        let (_tmp, mut ws) = blank_sheet();
        ws.start_placing(&CellId::new("std", "resistor"));
        // Shift+click keeps placing; a plain click places the last one and goes back
        // to Select.
        let shift_click = |ws: &mut Workspace, x, y| {
            ws.gesture(Gesture::Click { at: Pt::new(x, y), shift: true }, 0.5)
        };
        assert!(shift_click(&mut ws, 0.0, 0.0).is_none());
        assert!(matches!(ws.tool, Tool::Place { .. }));
        assert!(click(&mut ws, 20.3, 0.1).is_none(), "snaps to (20.32, 0)");
        assert_eq!(ws.tool, Tool::Select);
        assert_eq!(ws.selection, BTreeSet::from([Owner::Instance(1)]), "the new part is selected");
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances.len(), 2);
        assert_eq!(s.instances[1].name, "R2");
        assert_eq!(s.instances[0].cell.to_string(), "std:resistor");

        // Wire from R1.b (5.08, 0) to R2.a (15.24, 0): the second click lands on a pin
        // and finishes the wire by itself.
        ws.tool = Tool::Wire { points: vec![], horizontal_first: true };
        click(&mut ws, 5.08, 0.0);
        click(&mut ws, 15.24, 0.0);
        assert_eq!(ws.tool, Tool::Wire { points: vec![], horizontal_first: true });
        let conn = &ws.scene.connectivity;
        assert_eq!(ws.schematic().unwrap().wires.len(), 1);
        assert_eq!(conn.dangling_pins.len(), 2, "only the outer pins remain open");
        assert!(ws.dirty);
    }

    #[test]
    fn wire_with_corners_ends_on_double_click() {
        let (_tmp, mut ws) = blank_sheet();
        ws.tool = Tool::Wire { points: vec![], horizontal_first: true };
        click(&mut ws, 0.0, 0.0);
        click(&mut ws, 10.16, 5.08); // L-route: (0,0) → (10.16,0) → (10.16,5.08)
        ws.gesture(Gesture::DoubleClick { at: Pt::new(10.16, 5.08) }, 0.5);
        let w = &ws.schematic().unwrap().wires[0].points;
        assert_eq!(w, &vec![Point::mm(0.0, 0.0), Point::mm(10.16, 0.0), Point::mm(10.16, 5.08)]);
    }

    #[test]
    fn dragging_a_part_moves_it_whatever_tool_is_in_hand() {
        for placing in [true, false] {
            let (_tmp, mut ws) = example_copy();
            let u1 = instance(&ws, "U1");
            let count = ws.schematic().unwrap().instances.len();
            if placing {
                ws.start_placing(&CellId::new("std", "resistor"));
                assert!(matches!(ws.tool, Tool::Place { .. }));
            } else {
                ws.tool = Tool::Wire { points: vec![], horizontal_first: true };
            }
            let from = Pt::new(35.56, 0.0);
            ws.gesture(Gesture::DragStart { from }, 0.5);
            ws.gesture(Gesture::Drag { from, to: Pt::new(35.56, 2.6) }, 0.5);
            ws.gesture(Gesture::DragEnd { from, to: Pt::new(35.56, 2.6), shift: false }, 0.5);
            assert_eq!(ws.tool, Tool::Select);
            let s = ws.schematic().unwrap();
            assert_eq!(s.instances.len(), count, "nothing placed");
            assert!(s.wires.iter().all(|w| w.points.len() >= 2));
            assert_eq!(s.instances[u1].placement.position, Point::mm(35.56, 2.54));
        }
    }

    #[test]
    fn dragging_moves_parts_and_is_one_undo_step() {
        let (_tmp, mut ws) = example_copy();
        let u1 = instance(&ws, "U1");
        let before = ws.current_def().unwrap().clone();
        ws.gesture(Gesture::DragStart { from: Pt::new(35.56, 0.0) }, 0.5);
        for y in [1.0, 2.0, 2.6] {
            let to = Pt::new(35.56, y);
            ws.gesture(Gesture::Drag { from: Pt::new(35.56, 0.0), to }, 0.5);
        }
        ws.gesture(
            Gesture::DragEnd { from: Pt::new(35.56, 0.0), to: Pt::new(35.56, 2.6), shift: false },
            0.5,
        );
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances[u1].placement.position, Point::mm(35.56, 2.54));
        // The inverter's wires followed it, so nothing came unconnected.
        assert!(ws.scene.connectivity.dangling_pins.is_empty());
        assert!(ws.can_undo());
        ws.undo();
        assert_eq!(ws.current_def().unwrap(), &before, "one undo restores the sheet");
        ws.redo();
        assert_eq!(
            ws.schematic().unwrap().instances[u1].placement.position,
            Point::mm(35.56, 2.54)
        );
    }

    #[test]
    fn box_select_then_delete() {
        let (_tmp, mut ws) = example_copy();
        ws.gesture(Gesture::DragStart { from: Pt::new(-5.0, -10.0) }, 0.5);
        ws.gesture(
            Gesture::DragEnd { from: Pt::new(-5.0, -10.0), to: Pt::new(5.0, 10.0), shift: false },
            0.5,
        );
        // The supply, its ground symbol and the wires around them.
        assert!(ws.selection.contains(&Owner::Instance(instance(&ws, "VDD"))));
        assert!(ws.selection.contains(&Owner::Instance(instance(&ws, "GND1"))));
        // ...and the wire stubs and VDD label around them, but not the clock next door.
        assert!(!ws.selection.contains(&Owner::Instance(instance(&ws, "VCLK"))));
        let s = ws.schematic().unwrap();
        let (n_inst, n_wires, n_labels) = (s.instances.len(), s.wires.len(), s.labels.len());
        ws.delete_selection();
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances.len(), n_inst - 2);
        assert_eq!(s.wires.len(), n_wires - 2);
        assert_eq!(s.labels.len(), n_labels - 1);
        assert!(ws.selection.is_empty());
    }

    #[test]
    fn cannot_place_a_cell_inside_itself() {
        let mut ws = example();
        let u1 = instance(&ws, "U1");
        ws.descend(u1);
        assert!(!ws.placeable(&CellId::new("cmos_inverter", "inverter")), "itself");
        assert!(!ws.placeable(&CellId::new("cmos_inverter", "testbench")), "contains it");
        assert!(ws.placeable(&CellId::new("std", "nmos")));
    }

    #[test]
    fn ports_labels_and_save_round_trip() {
        let (tmp, mut ws) = blank_sheet();
        ws.tool = Tool::Port { name: "in".into(), kind: PortKind::Input };
        assert!(click(&mut ws, 0.0, 0.0).is_none());
        ws.tool = Tool::Label { net: "MID".into() };
        click(&mut ws, 10.16, 0.0);
        assert_eq!(ws.current_def().unwrap().cell.ports.len(), 1);
        ws.save().unwrap();
        assert!(!ws.dirty);

        let reopened = Workspace::open(tmp.path()).unwrap();
        let def = reopened.design.cell(&CellId::new("cmos_inverter", "scratch")).unwrap();
        let s = def.schematic.as_ref().unwrap();
        assert_eq!((s.ports.len(), s.labels.len()), (1, 1));
        assert_eq!(def.cell.ports[0].kind, PortKind::Input);
    }

    #[test]
    fn overlays_follow_the_tool() {
        let (_tmp, mut ws) = blank_sheet();
        let cursor = Some(Pt::new(10.0, 10.1));
        assert!(ws.overlay(cursor).ghost.is_empty());

        ws.start_placing(&CellId::new("std", "resistor"));
        let o = ws.overlay(cursor);
        assert_eq!(o.snap, Some(Pt::new(10.16, 10.16)));
        assert!(!o.ghost.is_empty(), "the resistor follows the cursor");
        let text: Vec<_> = o
            .ghost
            .iter()
            .filter_map(|e| match &e.shape {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains(&"R1"), "ghost shows the name it will get: {text:?}");

        ws.tool = Tool::Wire { points: vec![Point::ORIGIN], horizontal_first: true };
        assert_eq!(
            ws.overlay(cursor).wire,
            vec![Pt::new(0.0, 0.0), Pt::new(10.16, 0.0), Pt::new(10.16, 10.16)]
        );

        ws.tool = Tool::Select;
        ws.gesture(Gesture::DragStart { from: Pt::new(-1.0, -1.0) }, 0.5);
        assert_eq!(ws.overlay(cursor).band, Some((Pt::new(-1.0, -1.0), Pt::new(10.0, 10.1))));
    }

    #[test]
    fn escape_cancels_a_move_in_progress() {
        let (_tmp, mut ws) = example_copy();
        let before = ws.current_def().unwrap().clone();
        ws.gesture(Gesture::DragStart { from: Pt::new(35.56, 0.0) }, 0.5);
        ws.gesture(Gesture::Drag { from: Pt::new(35.56, 0.0), to: Pt::new(45.0, 0.0) }, 0.5);
        ws.escape();
        assert_eq!(ws.current_def().unwrap(), &before);
        assert!(!ws.can_undo());
    }

    fn scratch() -> CellId {
        CellId::new("cmos_inverter", "scratch")
    }

    #[test]
    fn edits_announce_what_they_connect_and_disconnect() {
        let (_tmp, mut ws) = blank_sheet();
        let r = CellId::new("std", "resistor");
        for x in [0.0, 20.32] {
            ws.start_placing(&r);
            click(&mut ws, x, 0.0);
        }
        ws.tool = Tool::Wire { points: Vec::new(), horizontal_first: true };
        click(&mut ws, 5.08, 0.0);
        click(&mut ws, 15.24, 0.0);
        assert_eq!(ws.notice.as_deref(), Some("Connected R1.b–R2.a"));
        ws.tool = Tool::Select;
        ws.select_only(Some(Owner::Wire(0)));
        ws.delete_selection();
        assert_eq!(ws.notice.as_deref(), Some("Disconnected R1.b–R2.a"));
        click(&mut ws, 100.0, 100.0);
        assert_eq!(ws.notice, None, "cleared by the next action");
    }

    #[test]
    fn imported_models_become_placeable_parts() {
        let (_tmp, mut ws) = example_copy();
        let lib = ".model D1N4001 D(IS=29.5n N=1.96)\n.subckt BUF in out\nE1 out 0 in 0 1\n.ends";
        let defs = circuiter_sim::parse::definitions(lib);
        let out = ws.import_models(&defs);
        assert_eq!(out.len(), 2);
        let names: Vec<String> = out.into_iter().map(|r| r.unwrap().0).collect();
        assert_eq!(names, ["D1N4001", "BUF"]);
        assert!(ws.dirty);
        for name in &names {
            assert!(ws.placeable(&CellId::new("cmos_inverter", name)), "{name}");
        }
    }

    /// Where pin `port` of instance `name` is on the current sheet.
    fn pin_at(ws: &Workspace, name: &str, port: &str) -> Pt {
        let i = instance(ws, name);
        let pin = ws
            .scene
            .connectivity
            .pins
            .iter()
            .find(|p| p.instance == i && p.port == port)
            .unwrap_or_else(|| panic!("{name}.{port}"));
        Pt::from(pin.at)
    }

    /// A wire dragged out of the pin end at `from` and let go at `to`, as by hand.
    fn drag_wire(ws: &mut Workspace, from: Pt, to: Pt) {
        assert!(ws.pin_end_at(from, 0.5).is_some(), "no pin end at {from:?}");
        ws.gesture(Gesture::DragStart { from }, 0.5);
        ws.gesture(Gesture::Drag { from, to }, 0.5);
        ws.gesture(Gesture::DragEnd { from, to, shift: false }, 0.5);
        assert_eq!(ws.tool, Tool::Select);
    }

    fn place(ws: &mut Workspace, cell: CellId, x: f64, y: f64) {
        ws.start_placing(&cell);
        assert!(matches!(ws.tool, Tool::Place { .. }), "{cell} isn't placeable here");
        assert!(click(ws, x, y).is_none());
    }

    /// The whole road from parts to a new part: draw a circuit in a new cell, give it
    /// ports, place the new part on another sheet and simulate the two together.
    #[test]
    fn a_component_built_from_parts_works_on_another_sheet() {
        let (_tmp, mut ws) = example_copy();
        let lib = ws.project.name().to_owned();
        let std = |name: &str| CellId::new("std", name);

        // A divider: in ─ R1 ─┬─ R2 ─ ground, out at the middle.
        ws.new_cell("half", "building").unwrap();
        place(&mut ws, std("resistor"), 0.0, 0.0);
        place(&mut ws, std("resistor"), 20.32, 0.0);
        place(&mut ws, std("gnd"), 30.48, 5.08);
        let (from, to) = (pin_at(&ws, "R1", "b"), pin_at(&ws, "R2", "a"));
        drag_wire(&mut ws, from, to);
        let (from, to) = (pin_at(&ws, "R2", "b"), pin_at(&ws, "GND1", "gnd"));
        drag_wire(&mut ws, from, to);
        let (from, to) = (pin_at(&ws, "R1", "a"), Pt::new(-10.16, 0.0));
        drag_wire(&mut ws, from, to);
        ws.tool = Tool::Port { name: "in".into(), kind: PortKind::Input };
        assert!(click(&mut ws, -10.16, 0.0).is_none());
        ws.tool = Tool::Port { name: "out".into(), kind: PortKind::Output };
        assert!(click(&mut ws, 10.16, 0.0).is_none(), "on the wire between R1 and R2");
        assert!(ws.scene.connectivity.dangling_pins.is_empty(), "{:?}", ws.problems());
        assert!(ws.problems().is_empty(), "{:?}", ws.problems());
        let half = CellId::new(&lib, "half");
        let def = ws.design.cell(&half).unwrap();
        let ports: Vec<&str> = def.cell.ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ports, ["in", "out"]);
        assert!(builder::is_generated(def), "a symbol was made from the ports");

        // Use it: a 10 V source into it, its output labelled OUT.
        ws.new_cell("top", "").unwrap();
        place(&mut ws, std("vdc"), -20.32, 0.0);
        place(&mut ws, half.clone(), 10.16, 0.0);
        let n = pin_at(&ws, "V1", "n");
        place(&mut ws, std("gnd"), n.x, n.y + 5.08);
        let g = pin_at(&ws, "GND1", "gnd");
        drag_wire(&mut ws, n, g);
        let (from, to) = (pin_at(&ws, "V1", "p"), pin_at(&ws, "X1", "in"));
        drag_wire(&mut ws, from, to);
        let out = pin_at(&ws, "X1", "out");
        let tail = Pt::new(out.x + 7.62, out.y);
        drag_wire(&mut ws, out, tail);
        ws.tool = Tool::Label { net: "OUT".into() };
        assert!(click(&mut ws, tail.x, tail.y).is_none());
        let v1 = instance(&ws, "V1");
        ws.edit(|_, _, d| {
            edit::set_param(d, v1, "V", Some("10".into()));
            Ok(())
        })
        .unwrap();
        assert!(ws.problems().iter().all(|(s, _)| *s == Severity::Warning), "{:?}", ws.problems());

        let top = CellId::new(&lib, "top");
        let mut sim = Simulation::prepare(&ws.design, &top).unwrap();
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let out = sim.netlist.net("OUT").expect("OUT net");
        let v = sim.voltage(out).unwrap();
        assert!((v - 5.0).abs() < 1e-6, "half of 10 V, got {v}");

        // And inside it, from the top: the part opens to the circuit it's made of.
        ws.descend(instance(&ws, "X1"));
        assert_eq!(ws.current().cell, half);
    }

    /// Build it flat, test it, then turn it into a part: the selection becomes a cell
    /// with ports for what it was connected to.
    #[test]
    fn selected_parts_become_a_component() {
        let (_tmp, mut ws) = example_copy();
        let lib = ws.project.name().to_owned();
        let std = |name: &str| CellId::new("std", name);
        // V1 → R1 → (OUT) → R2 → ground, flat on one sheet.
        ws.new_cell("bench", "").unwrap();
        place(&mut ws, std("vdc"), -20.32, 0.0);
        place(&mut ws, std("resistor"), 0.0, -10.16);
        place(&mut ws, std("resistor"), 20.32, -10.16);
        let n = pin_at(&ws, "V1", "n");
        place(&mut ws, std("gnd"), n.x, n.y + 5.08);
        place(&mut ws, std("gnd"), 30.48, -5.08);
        let (from, to) = (n, pin_at(&ws, "GND1", "gnd"));
        drag_wire(&mut ws, from, to);
        let (from, to) = (pin_at(&ws, "V1", "p"), pin_at(&ws, "R1", "a"));
        drag_wire(&mut ws, from, to);
        let (from, to) = (pin_at(&ws, "R1", "b"), pin_at(&ws, "R2", "a"));
        drag_wire(&mut ws, from, to);
        let (from, to) = (pin_at(&ws, "R2", "b"), pin_at(&ws, "GND2", "gnd"));
        drag_wire(&mut ws, from, to);
        ws.tool = Tool::Label { net: "OUT".into() };
        assert!(click(&mut ws, 10.16, -10.16).is_none());
        assert!(ws.scene.connectivity.dangling_pins.is_empty(), "{:?}", ws.problems());

        // Select the two resistors (and so the wire between them).
        ws.selection = BTreeSet::from([
            Owner::Instance(instance(&ws, "R1")),
            Owner::Instance(instance(&ws, "R2")),
        ]);
        assert!(ws.component_from_selection("bench", "").is_err(), "name taken");
        let (div, ports) = ws.component_from_selection("div", "blocks").unwrap();
        // R1.a went to V1: a port. The middle was labelled OUT: a port named so.
        // R2.b went to ground: a GND label, not a port.
        assert_eq!(ports, ["p1", "OUT"]);
        assert_eq!(ws.current().cell, div, "opened");
        assert!(ws.notice.as_deref().is_some_and(|n| n.contains("Made div")));
        let def = ws.design.cell(&div).unwrap();
        assert_eq!(def.folder, "blocks");
        assert!(builder::is_generated(def));
        let s = def.schematic.as_ref().unwrap();
        assert_eq!(s.instances.len(), 2);
        assert!(s.labels.iter().any(|l| l.net == "GND"));
        assert!(ws.scene.connectivity.dangling_pins.is_empty(), "{:?}", ws.problems());
        assert!(ws.problems().is_empty(), "{:?}", ws.problems());
        // The sheet it came from is untouched.
        let bench = CellId::new(&lib, "bench");
        assert_eq!(ws.design.cell(&bench).unwrap().schematic.as_ref().unwrap().instances.len(), 5);

        // It works as a part: 10 V in, 5 V out.
        ws.new_cell("top", "").unwrap();
        place(&mut ws, std("vdc"), -20.32, 0.0);
        place(&mut ws, div.clone(), 10.16, 0.0);
        let n = pin_at(&ws, "V1", "n");
        place(&mut ws, std("gnd"), n.x, n.y + 5.08);
        let g = pin_at(&ws, "GND1", "gnd");
        drag_wire(&mut ws, n, g);
        let (from, to) = (pin_at(&ws, "V1", "p"), pin_at(&ws, "X1", "p1"));
        drag_wire(&mut ws, from, to);
        let out = pin_at(&ws, "X1", "OUT");
        let tail = Pt::new(out.x + 7.62, out.y);
        drag_wire(&mut ws, out, tail);
        ws.tool = Tool::Label { net: "Y".into() };
        assert!(click(&mut ws, tail.x, tail.y).is_none());
        let v1 = instance(&ws, "V1");
        ws.edit(|_, _, d| {
            edit::set_param(d, v1, "V", Some("10".into()));
            Ok(())
        })
        .unwrap();
        let mut sim = Simulation::prepare(&ws.design, &CellId::new(&lib, "top")).unwrap();
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let v = sim.voltage(sim.netlist.net("Y").unwrap()).unwrap();
        assert!((v - 5.0).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn a_new_cell_becomes_a_part_once_it_has_ports() {
        let (_tmp, mut ws) = blank_sheet();
        ws.tool = Tool::Port { name: "a".into(), kind: PortKind::Input };
        click(&mut ws, 0.0, 0.0);
        ws.tool = Tool::Port { name: "y".into(), kind: PortKind::Output };
        click(&mut ws, 10.16, 0.0);
        let def = ws.current_def().unwrap();
        assert!(builder::is_generated(def), "symbol generated from the ports");
        assert!(ws.problems().iter().all(|(_, m)| !m.contains("pin")), "{:?}", ws.problems());

        ws.open_cell(CellId::new("cmos_inverter", "testbench"));
        assert!(ws.placeable(&scratch()));
        ws.start_placing(&scratch());
        assert!(click(&mut ws, 50.8, 50.8).is_none());
        assert!(ws.problems().iter().all(|(s, _)| *s == Severity::Warning));
    }

    #[test]
    fn interface_edits_keep_a_generated_symbol_in_step() {
        let (_tmp, mut ws) = blank_sheet();
        ws.set_mode(Mode::Symbol);
        assert_eq!(ws.mode, Mode::Symbol);
        ws.edit(|_, _, d| crate::interface::add_port(d, "din", PortKind::Input)).unwrap();
        ws.edit(|_, _, d| crate::interface::add_port(d, "q", PortKind::Output)).unwrap();
        ws.edit(|_, _, d| crate::interface::rename_port(d, "din", "data_in")).unwrap();
        let def = ws.current_def().unwrap();
        assert!(builder::is_generated(def));
        assert!(def.symbol.as_ref().unwrap().pin("data_in").is_some());
        // The symbol view shows each pin as its own object.
        assert!(ws.scene.elements.iter().any(|e| e.owner == Owner::Pin(0)));
        ws.undo();
        assert!(ws.current_def().unwrap().symbol.as_ref().unwrap().pin("din").is_some());
    }

    #[test]
    fn draw_move_and_rotate_on_a_symbol() {
        let (_tmp, mut ws) = blank_sheet();
        ws.edit(|_, _, d| crate::interface::add_port(d, "a", PortKind::Input)).unwrap();
        ws.set_mode(Mode::Symbol);
        let graphics = |ws: &Workspace| ws.symbol().unwrap().graphics.len();
        let before = graphics(&ws);

        ws.tool = Tool::Rect { from: None };
        click(&mut ws, -1.27, -1.27);
        click(&mut ws, 1.27, 1.27);
        assert_eq!(graphics(&ws), before + 1);
        ws.tool = Tool::Line { points: Vec::new() };
        for (x, y) in [(0.0, 0.0), (1.27, 0.0), (1.27, 1.27), (0.0, 0.0)] {
            click(&mut ws, x, y);
        }
        assert_eq!(graphics(&ws), before + 2, "clicking the start closes the line");
        let Graphic::Polyline { points, .. } = &ws.symbol().unwrap().graphics[before + 1] else {
            panic!("not a polyline")
        };
        assert_eq!(points.len(), 4);

        // Drag the pin: it stays on the 100-mil grid.
        ws.tool = Tool::Select;
        let pin = ws.symbol().unwrap().pins[0].at;
        let from = Pt::from(pin);
        let to = Pt::new(from.x - 3.0, from.y + 1.0);
        ws.gesture(Gesture::DragStart { from }, 0.5);
        ws.gesture(Gesture::Drag { from, to }, 0.5);
        ws.gesture(Gesture::DragEnd { from, to, shift: false }, 0.5);
        let moved = ws.symbol().unwrap().pins[0].at;
        assert_eq!(moved, Point::new(pin.x - GRID, pin.y));
        assert!(!builder::is_generated(ws.current_def().unwrap()), "now a custom drawing");

        ws.select_only(Some(Owner::Pin(0)));
        ws.rotate();
        assert_eq!(ws.symbol().unwrap().pins[0].direction, PinDirection::Up);
        ws.delete_selection();
        assert!(ws.symbol().unwrap().pins.is_empty());
        assert!(ws.problems().iter().any(|(_, m)| m.contains("has no pin")));

        // The pin tool puts it back, then returns to selecting.
        ws.start_pin(None).unwrap();
        assert!(matches!(&ws.tool, Tool::Pin { port, .. } if port == "a"));
        assert!(click(&mut ws, -7.62, 0.0).is_none());
        assert_eq!(ws.symbol().unwrap().pins.len(), 1);
        assert_eq!(ws.tool, Tool::Select);
        assert!(ws.start_pin(None).is_err(), "nothing left to pin");
    }

    #[test]
    fn a_custom_symbol_gets_pins_for_new_ports() {
        let (_tmp, mut ws) = blank_sheet();
        ws.edit(|_, _, d| crate::interface::add_port(d, "a", PortKind::Input)).unwrap();
        ws.set_mode(Mode::Symbol);
        ws.tool = Tool::Circle { center: None };
        click(&mut ws, 0.0, 0.0);
        click(&mut ws, 1.27, 0.0);
        ws.edit(|_, _, d| crate::interface::add_port(d, "b", PortKind::Input)).unwrap();
        let s = ws.symbol().unwrap();
        assert!(s.graphics.iter().any(|g| matches!(g, Graphic::Circle { .. })), "drawing kept");
        let (a, b) = (s.pin("a").unwrap().at, s.pin("b").unwrap().at);
        assert_eq!((b.x, b.y), (a.x, a.y + GRID), "new pin below the last on its side");
    }

    #[test]
    fn library_symbols_can_be_viewed_but_not_edited() {
        let mut ws = example();
        ws.open_cell(CellId::new("std", "resistor"));
        ws.set_mode(Mode::Schematic);
        assert_eq!(ws.mode, Mode::Symbol, "no schematic to switch to");
        assert!(ws.scene.elements.iter().any(|e| matches!(e.owner, Owner::Pin(_))));
        ws.select_only(Some(Owner::Pin(0)));
        ws.delete_selection();
        assert_eq!(ws.symbol().unwrap().pins.len(), 2);
    }
}
