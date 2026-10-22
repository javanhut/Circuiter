//! The tools an assistant can call: what each does, the arguments it takes (as a JSON
//! schema), and the work. Each call reads the project from disk, and a tool that changes
//! it writes it back before answering.

use crate::edit::{self, GRID};
use crate::interface;
use crate::simulation::{self, Analysis, Results, Simulation};
use circuiter_model::board::{Placed, Zone};
use circuiter_model::builder;
use circuiter_model::cell::{
    Cell, ParamDef, ParamKind, Port, PortKind, validate_name, validate_port_name,
};
use circuiter_model::check::{Diagnostic, Severity, check_cell, check_libraries};
use circuiter_model::connectivity::{Connectivity, Item, NetNameSource};
use circuiter_model::schematic::{Frame, Instance, Note, Paper, Schematic, SchematicPort};
use circuiter_model::symbol::PinDirection;
use circuiter_model::{
    CellDef, CellId, CellRef, Design, Library, Point, Project, Rotation, STD_LIBRARY, Transform,
    from_ron, to_ron,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// The state kept between calls: which project, and the standard library, read once.
#[derive(Debug)]
pub struct Server {
    root: Option<PathBuf>,
    std: Library,
}

type Run = fn(&mut Server, &Value) -> Result<String, String>;

struct Tool {
    name: &'static str,
    description: &'static str,
    input: Value,
    run: Run,
}

const NO_PROJECT: &str =
    "no project is open: open_project or create_project first, or start the server in one";
const CELL: &str = "The cell: its name in this project, or `library:cell`";
const CELL_OR_TOP: &str =
    "The cell: its name in this project, or `library:cell` (default: the project's top cell)";
const VIEWS: &str = "Views to set, each as RON text in the format of the cell's files, or \
null to remove the view: `symbol`, `schematic`, `spice`, `digital`, `footprint`, `model3d`, \
`board`, or `cell` (the whole interface). get_cell shows the format; read a similar \
standard part first.";

fn tools() -> Vec<Tool> {
    let port = json!({
        "type": "object",
        "properties": {
            "name": string("Port name: letters, digits and underscores"),
            "kind": string("input, output, bidirectional, passive (default), power_in or power_out"),
            "description": string("What the port is for"),
            "group": string("Section of the generated symbol it is drawn in (`Power`, `Port A`)"),
            "width": json!({ "type": "integer", "description": "Bits, for a bus (default 1)" }),
        },
        "required": ["name"],
    });
    let param = json!({
        "type": "object",
        "properties": {
            "name": string("Parameter name"),
            "kind": string("real (a quantity), integer, bool, text or choice"),
            "unit": string("Unit of a real parameter: `Ω`, `F`, `V`"),
            "choices": strings("The options of a choice parameter"),
            "default": string("Default value, such as `10k`"),
            "description": string("What it sets"),
        },
        "required": ["name"],
    });
    let interface = |extra: Value, required: &[&str]| {
        let mut props = json!({
            "description": string("What the component is, for people and for search_parts"),
            "prefix": string("Reference prefix: `R`, `U`, `Q` (default `U`)"),
            "folder": string("Folder in the project's library, such as `analog/opamps`"),
            "ports": json!({ "type": "array", "items": port, "description": "The ports, in order" }),
            "params": json!({ "type": "array", "items": param, "description": "Parameters set on each part placed" }),
            "views": json!({ "type": "object", "description": VIEWS, "additionalProperties": { "type": ["string", "null"] } }),
        });
        for (k, v) in extra.as_object().expect("object") {
            props[k] = v.clone();
        }
        object(props, required)
    };
    let part = json!({
        "type": "object",
        "properties": {
            "part": string("The part: `std:resistor`, a project cell's name, or `library:cell`"),
            "name": string("Reference such as `R1` (default: the next free one)"),
            "at": point("Where its origin goes, in mm (default: beside what is there)"),
            "rotation": json!({ "type": "integer", "enum": [0, 90, 180, 270], "description": "Degrees counter-clockwise" }),
            "mirror": boolean("Mirrored left to right"),
            "params": json!({ "type": "object", "description": "Parameter values, such as {\"R\": \"10k\"}", "additionalProperties": { "type": "string" } }),
        },
        "required": ["part"],
    });
    vec![
        Tool {
            name: "open_project",
            description: "Opens the Circuiter project in a directory (the one holding circuiter.toml, or inside it) for the other tools to work on.",
            input: object(json!({ "path": string("The project's directory") }), &["path"]),
            run: open_project,
        },
        Tool {
            name: "create_project",
            description: "Makes a new project in a directory (made if missing) with an empty top-level schematic, and opens it.",
            input: object(
                json!({
                    "path": string("Directory for the project"),
                    "name": string("Project name (default: from the directory's name)"),
                }),
                &["path"],
            ),
            run: create_project,
        },
        Tool {
            name: "project_info",
            description: "The open project: its name, top cell, libraries, and every cell with its description, views and ports.",
            input: object(json!({}), &[]),
            run: project_info,
        },
        Tool {
            name: "search_parts",
            description: "Finds parts to use, in the standard library and the project's: each with its ports, parameters and views. Every word of the query must match the name, folder, description or a port.",
            input: object(
                json!({
                    "query": string("Words to look for, such as `op amp` or `regulator 3.3`; empty lists everything"),
                    "library": string("Only this library, such as `std`"),
                    "limit": json!({ "type": "integer", "description": "At most this many (default 40)" }),
                }),
                &[],
            ),
            run: search_parts,
        },
        Tool {
            name: "get_cell",
            description: "A cell's views as the RON text of its files: `cell` (interface), `symbol`, `schematic`, `spice`, `digital`, `footprint`, `model3d`, `board`. Use it to see how a standard part is made before writing views, or to read a schematic exactly.",
            input: object(
                json!({
                    "cell": string(CELL),
                    "views": strings("Which views (default: all but the board)"),
                }),
                &["cell"],
            ),
            run: get_cell,
        },
        Tool {
            name: "create_component",
            description: "Makes a new component (cell) in the project: its ports and parameters, a symbol drawn from the ports unless one is given, and any views given as RON, such as a SPICE model (`spice`), a footprint (`footprint`: a package generator and its parameters) or a 3D body (`model3d`). With `schematic: true` it is a circuit block instead, its ports placed on an empty schematic to wire up with place_parts and connect.",
            input: interface(
                json!({
                    "name": string("Cell name: letters, digits and underscores, starting with a letter"),
                    "schematic": boolean("Give it an empty schematic with its ports placed, to build it from parts"),
                }),
                &["name"],
            ),
            run: create_component,
        },
        Tool {
            name: "update_component",
            description: "Changes a project cell: its description, prefix or folder, its ports or parameters (each given as the complete new list), or any view as RON. A symbol drawn from the ports follows them; `regenerate_symbol` redraws it from scratch.",
            input: interface(
                json!({
                    "cell": string(CELL),
                    "regenerate_symbol": boolean("Draw the symbol afresh from the ports"),
                }),
                &["cell"],
            ),
            run: update_component,
        },
        Tool {
            name: "delete_cell",
            description: "Deletes a project cell. Refuses while other cells use it, unless forced.",
            input: object(
                json!({ "cell": string(CELL), "force": boolean("Delete it even if parts of it are placed elsewhere") }),
                &["cell"],
            ),
            run: delete_cell,
        },
        Tool {
            name: "set_top",
            description: "Makes a project cell the top of the design: what is simulated and laid out by default.",
            input: object(json!({ "cell": string(CELL) }), &["cell"]),
            run: set_top,
        },
        Tool {
            name: "import_spice",
            description: "Imports manufacturer SPICE models: each `.subckt` becomes a component with a pin per node, and each `.model` of a diode or transistor a part like the standard one. The simulator tries each at once and says if it can't run it.",
            input: object(
                json!({ "text": string("The model text: .subckt … .ends and .model cards") }),
                &["text"],
            ),
            run: import_spice,
        },
        Tool {
            name: "describe_schematic",
            description: "A schematic as parts and nets: every part with its position, rotation and parameters; every net with the pins on it; pins left unconnected; and wiring mistakes.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "pins": boolean("Also list where every pin is and which way its wire leaves"),
                }),
                &[],
            ),
            run: describe_schematic,
        },
        Tool {
            name: "place_parts",
            description: "Places parts on a cell's schematic (making the schematic if it has none). Returns each part's pins: where they are and which way a wire leaves them. Parts placed without `at` go in a row beside the rest; arrange_sheet tidies everything later.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "parts": json!({ "type": "array", "items": part }),
                }),
                &["parts"],
            ),
            run: place_parts,
        },
        Tool {
            name: "connect",
            description: "Joins pins into a named net. Each pin gets a short wire and a label with the net's name; same-named labels are connected. Pins are `R1.a` (part and port), or `port:IN` for the sheet's own port IN. Supply and ground nets (`GND`, `+5V`, `VCC`) are made global, so they reach every sheet. Afterwards arrange_sheet replaces the labels with drawn wires.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "net": string("The net's name, such as `VOUT` or `GND`"),
                    "pins": strings("Pins to join: `R1.a`, `U1.out`, `port:IN`"),
                    "global": boolean("Make the net global, reaching every sheet (automatic for supply names)"),
                }),
                &["net", "pins"],
            ),
            run: connect,
        },
        Tool {
            name: "disconnect",
            description: "Takes pins off their nets: removes the wires ending on them and the labels on them.",
            input: object(
                json!({ "cell": string(CELL_OR_TOP), "pins": strings("Pins such as `R1.a`") }),
                &["pins"],
            ),
            run: disconnect,
        },
        Tool {
            name: "edit_parts",
            description: "Changes placed parts: parameters, reference, position, rotation, mirroring. A part that moves keeps its connections: labels made by connect come with it, and wires drawn to it stay where they were, joined to it by a label of the net's name (arrange_sheet redraws them).",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "parts": json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": string("The part's reference, such as `R1`"),
                                "rename": string("A new reference"),
                                "at": point("New position, mm"),
                                "rotation": json!({ "type": "integer", "enum": [0, 90, 180, 270] }),
                                "mirror": boolean("Mirrored"),
                                "params": json!({ "type": "object", "description": "Values to set; null resets one to its default", "additionalProperties": { "type": ["string", "null"] } }),
                            },
                            "required": ["name"],
                        },
                    }),
                }),
                &["parts"],
            ),
            run: edit_parts,
        },
        Tool {
            name: "delete_parts",
            description: "Removes parts from a schematic, with the labels connect gave their pins and any wire that only led to them.",
            input: object(
                json!({ "cell": string(CELL_OR_TOP), "names": strings("References such as `R1`") }),
                &["names"],
            ),
            run: delete_parts,
        },
        Tool {
            name: "draw",
            description: "Draws on a schematic directly: wires through points, net labels, the cell's ports as flags, no-connect flags, and notes (text, or a dashed box with a title around a group of parts). Coordinates are mm on the 2.54 mm grid; things connect where their points meet.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "wires": json!({ "type": "array", "items": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } }, "description": "Each wire as its points, [[x, y], …]" }),
                    "labels": json!({ "type": "array", "items": { "type": "object", "properties": { "net": string("Net name"), "at": point("Where, on a wire end or pin"), "global": boolean("Global") }, "required": ["net", "at"] } }),
                    "ports": json!({ "type": "array", "items": { "type": "object", "properties": { "name": string("A port of the cell"), "at": point("Where") }, "required": ["name", "at"] } }),
                    "no_connects": json!({ "type": "array", "items": { "type": "array", "items": { "type": "number" } }, "description": "Pins meant to be left open, by position" }),
                    "notes": json!({ "type": "array", "items": { "type": "object", "properties": { "text": string("Note text (lines split on \\n)"), "at": point("Top-left of a text note"), "title": string("Title of a group box"), "from": point("One corner of a group box"), "to": point("The other corner") } } }),
                }),
                &[],
            ),
            run: draw,
        },
        Tool {
            name: "arrange_sheet",
            description: "Lays a schematic out afresh as Circuiter's Diagram view does: parts placed left to right by signal flow, every net drawn as wires, ground and supply symbols where they belong. The nets stay the same. Notes are kept below the drawing; group boxes are dropped.",
            input: object(json!({ "cell": string(CELL_OR_TOP) }), &[]),
            run: arrange_sheet,
        },
        Tool {
            name: "set_frame",
            description: "Puts a drawing frame with a title block around a schematic, on the smallest paper that holds it, and fills in the title block.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "title": string("Title (default: the cell's name)"),
                    "revision": string("Revision"),
                    "date": string("Date"),
                    "company": string("Company"),
                    "comment": string("A line of comment"),
                    "remove": boolean("Take the frame off"),
                }),
                &[],
            ),
            run: set_frame,
        },
        Tool {
            name: "check",
            description: "Checks the design: the libraries' cells (missing views, bad templates, symbols out of step) and the electrical rules of a cell's schematic and everything under it (unconnected inputs, outputs driving each other, nets with nothing to drive them).",
            input: object(json!({ "cell": string(CELL_OR_TOP) }), &[]),
            run: check,
        },
        Tool {
            name: "simulate",
            description: "Simulates a cell's schematic. `op`: the DC operating point (every net's voltage, every part's currents and power). `tran`: a transient run, giving each probed net's range, final value and samples (digital designs run in the logic simulator and report value changes). `ac`: a small-signal frequency sweep, giving gain and phase. Parts that overheat or are overstressed are reported.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "analysis": string("op, tran or ac"),
                    "tstop": json!({ "type": ["number", "string"], "description": "tran: end time, s (`10m` works); default from the circuit" }),
                    "tstep": json!({ "type": ["number", "string"], "description": "tran: step, s" }),
                    "power_on": boolean("tran: start with everything off, as at switch-on, instead of from the operating point"),
                    "fstart": json!({ "type": ["number", "string"], "description": "ac: first frequency, Hz (default 1)" }),
                    "fstop": json!({ "type": ["number", "string"], "description": "ac: last frequency, Hz (default 1 MHz)" }),
                    "per_decade": json!({ "type": "integer", "description": "ac: points per decade (default 20)" }),
                    "probes": strings("Nets to report (default: the circuit's main nets); `U1.mid` for one inside a block"),
                }),
                &["analysis"],
            ),
            run: simulate,
        },
        Tool {
            name: "pcb_new_board",
            description: "Starts a printed circuit board for a cell's schematic: a board sized for its parts with every footprint placed, the placement optimised for short connections. Parts need footprints (most standard parts have them).",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "layers": json!({ "type": "integer", "enum": [2, 4, 6], "description": "Copper layers (default 2)" }),
                    "replace": boolean("Throw away the board the cell has and start again"),
                }),
                &[],
            ),
            run: pcb_new_board,
        },
        Tool {
            name: "pcb_update",
            description: "Brings a board in line with its schematic: places footprints of new parts, removes those of deleted parts, and takes up routing of nets that are gone.",
            input: object(json!({ "cell": string(CELL_OR_TOP) }), &[]),
            run: pcb_update,
        },
        Tool {
            name: "pcb_place",
            description: "Moves footprints on a board. Tracks ending on their pads come along.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "parts": json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "ref": string("The part, such as `U1` (or `X1.R2` inside a block)"),
                                "at": point("Centre, mm"),
                                "rotation": json!({ "type": "number", "description": "Degrees counter-clockwise" }),
                                "back": boolean("On the back of the board"),
                                "locked": boolean("Keep the optimiser from moving it"),
                            },
                            "required": ["ref", "at"],
                        },
                    }),
                }),
                &["parts"],
            ),
            run: pcb_place,
        },
        Tool {
            name: "pcb_autoroute",
            description: "Routes the board's unrouted connections automatically, of the nets given or all of them. Says which it couldn't route.",
            input: object(
                json!({ "cell": string(CELL_OR_TOP), "nets": strings("Only these nets") }),
                &[],
            ),
            run: pcb_autoroute,
        },
        Tool {
            name: "pcb_pour",
            description: "Pours copper for a net (usually GND) over the whole board on the layers given, keeping clear of other nets.",
            input: object(
                json!({
                    "cell": string(CELL_OR_TOP),
                    "net": string("The net, such as `GND`"),
                    "layers": strings("Copper layers: `F.Cu`, `B.Cu`, `In1.Cu`… (default the outer two)"),
                }),
                &["net"],
            ),
            run: pcb_pour,
        },
        Tool {
            name: "pcb_check",
            description: "The design rules check of a board: clearances, shorts, unrouted connections, parts off the board or overlapping, and 3D bodies that collide.",
            input: object(json!({ "cell": string(CELL_OR_TOP) }), &[]),
            run: pcb_check,
        },
        Tool {
            name: "list_files",
            description: "Lists the project's files, such as programs and HDL sources, under a directory of the project.",
            input: object(
                json!({ "path": string("Directory, relative to the project (default: the project)") }),
                &[],
            ),
            run: list_files,
        },
        Tool {
            name: "read_file",
            description: "Reads a text file of the project: a program, an HDL source, a cell's RON file.",
            input: object(json!({ "path": string("Path relative to the project") }), &["path"]),
            run: read_file,
        },
        Tool {
            name: "write_file",
            description: "Writes a text file in the project, such as a C or assembly program for a memory, or a SystemVerilog module. A design file (circuiter.toml, or under cells/) must leave the project readable, or it is put back as it was.",
            input: object(
                json!({ "path": string("Path relative to the project"), "text": string("The whole file") }),
                &["path", "text"],
            ),
            run: write_file,
        },
    ]
}

/// Every tool, as `tools/list` gives them.
pub fn list() -> Vec<Value> {
    tools()
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input }))
        .collect()
}

pub fn exists(name: &str) -> bool {
    tools().iter().any(|t| t.name == name)
}

impl Server {
    /// A server for the project at or above `project`, or else at or above the current
    /// directory, if there is one.
    pub fn new(project: Option<PathBuf>) -> anyhow::Result<Server> {
        let root = match project {
            Some(path) => Some(Project::find_root(&path).ok_or_else(|| {
                anyhow::anyhow!("no Circuiter project at or above {}", path.display())
            })?),
            None => std::env::current_dir().ok().and_then(|d| Project::find_root(&d)),
        };
        let root = root.map(|r| r.canonicalize().unwrap_or(r));
        Ok(Server { root, std: circuiter_stdlib::load()? })
    }

    pub fn call(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let tool = tools().into_iter().find(|t| t.name == name).ok_or("no such tool")?;
        (tool.run)(self, args)
    }

    fn session(&self) -> Result<Session, String> {
        let root = self.root.as_ref().ok_or(NO_PROJECT)?;
        let project = Project::load(root).map_err(|e| e.to_string())?;
        let design = Design::for_project(&project, self.std.clone()).map_err(|e| e.to_string())?;
        Ok(Session { project, design })
    }

    fn root(&self) -> Result<&Path, String> {
        self.root.as_deref().ok_or_else(|| NO_PROJECT.to_owned())
    }
}

/// The project as read from disk for one call.
struct Session {
    project: Project,
    design: Design,
}

impl Session {
    fn lib(&self) -> String {
        self.project.name().to_owned()
    }

    fn top(&self) -> CellId {
        CellId::new(self.project.name(), &self.project.manifest.project.top)
    }

    /// The cell named by the `cell` argument, or the top cell without one.
    fn id(&self, args: &Value) -> Result<CellId, String> {
        match opt(args, "cell") {
            None | Some("") => Ok(self.top()),
            Some(name) => self.find(name),
        }
    }

    fn find(&self, name: &str) -> Result<CellId, String> {
        let r: CellRef = name.trim().parse()?;
        let id = CellId::new(r.library.as_deref().unwrap_or(self.project.name()), &r.cell);
        if self.design.cell(&id).is_none() {
            return Err(format!(
                "no cell `{name}`: project_info lists the project's cells, search_parts finds parts"
            ));
        }
        Ok(id)
    }

    fn def(&self, id: &CellId) -> &CellDef {
        self.design.cell(id).expect("found")
    }

    fn editable(&self, id: &CellId) -> Result<(), String> {
        if id.library == STD_LIBRARY {
            return Err(format!(
                "{id} is in the standard library, which can't be changed: make a project \
                 component to change instead"
            ));
        }
        Ok(())
    }

    /// Changes cell `id` as the app's editor does: with `sync`, a symbol drawn from the
    /// ports follows them.
    fn edit<T>(
        &mut self,
        id: &CellId,
        sync: bool,
        f: impl FnOnce(&Design, &str, &mut CellDef) -> Result<T, String>,
    ) -> Result<T, String> {
        self.editable(id)?;
        let before = self.def(id).clone();
        let mut after = before.clone();
        let out = f(&self.design, &id.library, &mut after)?;
        if sync {
            builder::sync(&before, &mut after);
        }
        *self.design.cell_mut(id).expect("exists") = after;
        Ok(out)
    }

    fn save(&mut self) -> Result<(), String> {
        let name = self.lib();
        self.project.library =
            self.design.library(&name).cloned().ok_or("the project library is missing")?;
        self.project.save().map_err(|e| e.to_string())?;
        for (lib, entry) in &self.project.manifest.libraries {
            if let Some(l) = self.design.library(lib) {
                l.save_dir(&self.project.root.join(&entry.path)).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// What the design checks find wrong with cell `id`, as lines, or nothing.
    fn problems(&self, id: &CellId) -> String {
        let mut found = Vec::new();
        check_cell(&self.design, id, self.def(id), &mut found);
        diagnostics(&found)
    }
}

// Arguments.

fn opt<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    opt(args, key).ok_or_else(|| format!("`{key}` is needed, as a string"))
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn items<'a>(args: &'a Value, key: &str) -> &'a [Value] {
    args.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn names<'a>(args: &'a Value, key: &str) -> Result<Vec<&'a str>, String> {
    items(args, key)
        .iter()
        .map(|v| v.as_str().ok_or_else(|| format!("`{key}` is a list of strings")))
        .collect()
}

/// A number, or a quantity in engineering notation (`10m`, `1meg`).
fn quantity(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(Value::String(s)) => circuiter_sim::number::parse_number(s.trim())
            .map(Some)
            .ok_or_else(|| format!("`{key}`: `{s}` is not a number")),
        Some(_) => Err(format!("`{key}` is a number")),
    }
}

/// `[x, y]` in millimetres.
fn to_point(v: &Value) -> Result<Point, String> {
    let xy: Vec<f64> =
        v.as_array().map_or(Vec::new(), |a| a.iter().filter_map(Value::as_f64).collect());
    match xy[..] {
        [x, y] => Ok(Point::mm(x, y)),
        _ => Err(format!("`{v}` is not a point: give [x, y] in mm")),
    }
}

fn rotation(v: &Value) -> Result<Rotation, String> {
    match v.as_i64() {
        Some(0) => Ok(Rotation::R0),
        Some(90) => Ok(Rotation::R90),
        Some(180) => Ok(Rotation::R180),
        Some(270) => Ok(Rotation::R270),
        _ => Err(format!("rotation `{v}`: use 0, 90, 180 or 270")),
    }
}

// Schemas.

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn strings(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": description })
}

fn point(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": description })
}

// Output.

/// A number with up to three decimals and no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

fn at(p: Point) -> String {
    format!("({}, {})", num(p.x.mm()), num(p.y.mm()))
}

fn diagnostics(found: &[Diagnostic]) -> String {
    found.iter().map(|d| format!("- {d}\n")).collect()
}

fn views(def: &CellDef) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (has, name) in [
        (def.symbol.is_some(), "symbol"),
        (def.schematic.is_some(), "schematic"),
        (def.spice.is_some(), "spice"),
        (def.digital.is_some(), "digital"),
        (def.footprint.is_some(), "footprint"),
        (def.model3d.is_some(), "model3d"),
        (def.board.is_some(), "board"),
    ] {
        if has {
            out.push(name);
        }
    }
    out
}

fn kind_name(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Input => "input",
        PortKind::Output => "output",
        PortKind::Bidirectional => "bidirectional",
        PortKind::Passive => "passive",
        PortKind::PowerIn => "power_in",
        PortKind::PowerOut => "power_out",
    }
}

fn ports_text(cell: &Cell) -> String {
    if cell.ports.is_empty() {
        return "none".into();
    }
    let list: Vec<String> = cell
        .ports
        .iter()
        .map(|p| match p.kind {
            PortKind::Passive => p.name.clone(),
            k => format!("{} ({})", p.name, kind_name(k)),
        })
        .collect();
    list.join(", ")
}

fn param_text(p: &ParamDef) -> String {
    let mut s = p.name.clone();
    match &p.kind {
        ParamKind::Choice(options) => {
            let _ = write!(s, " [{}]", options.join("|"));
        }
        ParamKind::Real { unit } if !unit.is_empty() => {
            let _ = write!(s, " ({unit})");
        }
        _ => {}
    }
    if let Some(d) = &p.default {
        let _ = write!(s, " = {d}");
    }
    s
}

/// One line about a part, for listings.
fn part_line(id: &str, def: &CellDef) -> String {
    let mut line = format!("- {id}");
    if !def.folder.is_empty() {
        let _ = write!(line, " [{}]", def.folder);
    }
    if !def.cell.description.is_empty() {
        let _ = write!(line, ": {}", def.cell.description);
    }
    let _ = write!(line, ". Ports: {}", ports_text(&def.cell));
    if !def.cell.params.is_empty() {
        let params: Vec<String> = def.cell.params.iter().map(param_text).collect();
        let _ = write!(line, ". Params: {}", params.join(", "));
    }
    let v = views(def);
    if !v.is_empty() {
        let _ = write!(line, ". Views: {}", v.join(", "));
    }
    line
}

// Project tools.

fn open_project(s: &mut Server, a: &Value) -> Result<String, String> {
    let path = PathBuf::from(text(a, "path")?);
    let root = Project::find_root(&path).ok_or_else(|| {
        format!("no Circuiter project (circuiter.toml) at or above {}", path.display())
    })?;
    s.root = Some(root.canonicalize().unwrap_or(root));
    project_info(s, a)
}

fn create_project(s: &mut Server, a: &Value) -> Result<String, String> {
    let dir = PathBuf::from(text(a, "path")?);
    std::fs::create_dir_all(&dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let name = match opt(a, "name") {
        Some(n) => n.to_owned(),
        None => {
            let base = dir.file_name().and_then(|n| n.to_str()).unwrap_or("project");
            let mut name: String =
                base.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
                name.insert_str(0, "p_");
            }
            name
        }
    };
    Project::create(&dir, &name).map_err(|e| e.to_string())?;
    s.root = Some(dir.clone());
    Ok(format!(
        "Made project `{name}` in {} with an empty top-level schematic, cell `top`. Open it \
         in Circuiter with `circuiter {}` to watch the design take shape.",
        dir.display(),
        dir.display()
    ))
}

fn project_info(s: &mut Server, _: &Value) -> Result<String, String> {
    let session = s.session()?;
    let p = &session.project;
    let mut out = format!(
        "Project `{}` in {}. Top cell: {}.\n",
        p.name(),
        p.root.display(),
        p.manifest.project.top
    );
    if !p.manifest.project.description.is_empty() {
        let _ = writeln!(out, "{}", p.manifest.project.description);
    }
    let _ = writeln!(
        out,
        "Libraries: {} (the project's cells), std (standard parts: search_parts){}",
        p.name(),
        p.manifest
            .libraries
            .iter()
            .map(|(n, e)| format!(", {n} (at {})", e.path.display()))
            .collect::<String>()
    );
    let mut libs = vec![session.lib()];
    libs.extend(p.manifest.libraries.keys().cloned());
    for lib in libs {
        let Some(library) = session.design.library(&lib) else { continue };
        let _ = writeln!(out, "\nCells of {lib}:");
        if library.cells.is_empty() {
            let _ = writeln!(out, "(none)");
        }
        for def in library.cells.values() {
            let _ = writeln!(out, "{}", part_line(&def.name, def));
        }
    }
    Ok(out)
}

fn search_parts(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let query = opt(a, "query").unwrap_or("").to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let only = opt(a, "library");
    let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize;
    let mut hits = Vec::new();
    for lib in session.design.libraries() {
        if only.is_some_and(|l| l != lib.name) {
            continue;
        }
        for def in lib.cells.values() {
            let ports: Vec<&str> = def.cell.ports.iter().map(|p| p.name.as_str()).collect();
            let hay =
                format!("{} {} {} {}", def.name, def.folder, def.cell.description, ports.join(" "))
                    .to_lowercase();
            if !words.iter().all(|w| hay.contains(w)) {
                continue;
            }
            let name = def.name.to_lowercase();
            let description = def.cell.description.to_lowercase();
            // The name itself, then the query word for word in the description's first
            // sentence (what the part is), then a word of it in the name.
            let first = description.split(['.', ':']).next().unwrap_or("");
            let rank = if name == query.replace(' ', "_") {
                0
            } else if !query.is_empty() && first.contains(query.trim()) {
                1
            } else if words.iter().any(|w| name.contains(w)) {
                2
            } else {
                3
            };
            // Placeable parts first: a cell with no ports is a design, not a part.
            let design_only = def.cell.ports.is_empty() && def.cell.global_net.is_none();
            // The plainest part first: a short description is a generic part.
            let plain = first.len();
            hits.push((design_only, rank, plain, format!("{}:{}", lib.name, def.name), def));
        }
    }
    if hits.is_empty() {
        return Ok(format!(
            "Nothing matches `{query}`. Try fewer or other words, or make the part with \
             create_component."
        ));
    }
    hits.sort_by(|a, b| (a.0, a.1, a.2, &a.3).cmp(&(b.0, b.1, b.2, &b.3)));
    let total = hits.len();
    let mut out: String =
        hits.iter().take(limit).map(|(_, _, _, id, def)| part_line(id, def) + "\n").collect();
    if total > limit {
        let _ = writeln!(out, "… and {} more: narrow the query, or raise `limit`.", total - limit);
    }
    Ok(out)
}

fn get_cell(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let id = session.find(text(a, "cell")?)?;
    let def = session.def(&id);
    let asked = names(a, "views")?;
    let wanted: Vec<&str> = if asked.is_empty() {
        let mut v = vec!["cell"];
        v.extend(views(def).into_iter().filter(|v| *v != "board"));
        v
    } else {
        asked
    };
    let mut out = format!("Cell {id}");
    if !def.folder.is_empty() {
        let _ = write!(out, " in folder {}", def.folder);
    }
    out.push_str(".\n");
    for view in wanted {
        let file = Path::new(view);
        let ron = |r: circuiter_model::Result<String>| r.map_err(|e| e.to_string());
        let text = match view {
            "cell" => Some(ron(to_ron(file, &def.cell))?),
            "symbol" => def.symbol.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "schematic" => def.schematic.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "spice" => def.spice.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "digital" => def.digital.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "footprint" => def.footprint.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "model3d" => def.model3d.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            "board" => def.board.as_ref().map(|v| ron(to_ron(file, v))).transpose()?,
            other => return Err(format!("no view `{other}`")),
        };
        match text {
            Some(t) => {
                let _ = write!(out, "\n{view}:\n```ron\n{t}```\n");
            }
            None => {
                let _ = writeln!(out, "\n{view}: none");
            }
        }
    }
    if asked_board_hidden(def, a) {
        let _ = writeln!(out, "\n(It has a board too: ask for views [\"board\"] to see it.)");
    }
    Ok(out)
}

fn asked_board_hidden(def: &CellDef, a: &Value) -> bool {
    def.board.is_some() && items(a, "views").is_empty()
}

// Components.

fn parse_kind(text: &str) -> Result<PortKind, String> {
    let key: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    Ok(match key.to_ascii_lowercase().as_str() {
        "input" | "in" => PortKind::Input,
        "output" | "out" => PortKind::Output,
        "bidirectional" | "bidir" | "inout" | "io" => PortKind::Bidirectional,
        "passive" | "" => PortKind::Passive,
        "powerin" | "power" => PortKind::PowerIn,
        "powerout" | "supply" => PortKind::PowerOut,
        _ => {
            return Err(format!(
                "port kind `{text}`: use input, output, bidirectional, passive, power_in or power_out"
            ));
        }
    })
}

fn parse_ports(list: &[Value]) -> Result<Vec<Port>, String> {
    let mut ports: Vec<Port> = Vec::new();
    for v in list {
        let (name, spec) = match v {
            Value::String(n) => (n.as_str(), &Value::Null),
            Value::Object(_) => (text(v, "name")?, v),
            _ => return Err("each port is an object with a `name`".into()),
        };
        validate_port_name(name).map_err(|e| format!("port `{name}`: {e}"))?;
        if ports.iter().any(|p| p.name == name) {
            return Err(format!("port `{name}` is listed twice"));
        }
        let mut port = Port::new(name, parse_kind(opt(spec, "kind").unwrap_or(""))?);
        port.description = opt(spec, "description").unwrap_or("").to_owned();
        port.group = opt(spec, "group").unwrap_or("").to_owned();
        if let Some(w) = spec.get("width").and_then(Value::as_u64) {
            port.width = w.max(1) as u32;
        }
        ports.push(port);
    }
    Ok(ports)
}

fn parse_params(list: &[Value]) -> Result<Vec<ParamDef>, String> {
    let mut params: Vec<ParamDef> = Vec::new();
    for v in list {
        let name = text(v, "name")?;
        validate_name(name).map_err(|e| format!("parameter `{name}`: {e}"))?;
        if params.iter().any(|p| p.name == name) {
            return Err(format!("parameter `{name}` is listed twice"));
        }
        let default = opt(v, "default").map(str::to_owned);
        let choices: Vec<String> = names(v, "choices")?.into_iter().map(str::to_owned).collect();
        let unit = opt(v, "unit").unwrap_or("").to_owned();
        let kind = match opt(v, "kind").map(str::to_ascii_lowercase).as_deref() {
            Some("real" | "quantity" | "number") => ParamKind::Real { unit },
            Some("integer" | "int") => ParamKind::Integer,
            Some("bool" | "boolean") => ParamKind::Bool,
            Some("text" | "string") => ParamKind::Text,
            Some("choice") => ParamKind::Choice(choices),
            Some(other) => {
                return Err(format!(
                    "parameter `{name}`: kind `{other}`: use real, integer, bool, text or choice"
                ));
            }
            None if !choices.is_empty() => ParamKind::Choice(choices),
            None if !unit.is_empty()
                || default
                    .as_deref()
                    .is_some_and(|d| circuiter_model::units::parse_quantity(d, None).is_ok()) =>
            {
                ParamKind::Real { unit }
            }
            None => ParamKind::Text,
        };
        if let Some(d) = &default {
            kind.validate(d).map_err(|e| format!("parameter `{name}`: {e}"))?;
        }
        let description = opt(v, "description").unwrap_or("").to_owned();
        params.push(ParamDef { name: name.to_owned(), kind, default, description });
    }
    Ok(params)
}

/// Sets the views given as RON (null removes one). Returns whether a symbol was given.
fn apply_views(def: &mut CellDef, views: Option<&Value>) -> Result<bool, String> {
    let Some(views) = views.filter(|v| !v.is_null()) else { return Ok(false) };
    let views = views.as_object().ok_or("`views` maps view names to RON text")?;
    let mut symbol = false;
    for (name, value) in views {
        let text = match value {
            Value::Null => None,
            Value::String(t) => Some(t.as_str()),
            _ => return Err(format!("view `{name}`: give RON text, or null to remove it")),
        };
        let file = PathBuf::from(format!("{name}.ron"));
        fn read<T: serde::de::DeserializeOwned>(
            file: &Path,
            text: Option<&str>,
        ) -> Result<Option<T>, String> {
            text.map(|t| from_ron(file, t)).transpose().map_err(|e| e.to_string())
        }
        match name.as_str() {
            "cell" => {
                def.cell = read(&file, text)?.ok_or("the `cell` view can't be removed")?;
            }
            "symbol" => {
                def.symbol = read(&file, text)?;
                symbol = true;
            }
            "schematic" => def.schematic = read(&file, text)?,
            "spice" => def.spice = read(&file, text)?,
            "digital" => def.digital = read(&file, text)?,
            "footprint" => def.footprint = read(&file, text)?,
            "model3d" | "3d" => def.model3d = read(&file, text)?,
            "board" => def.board = read(&file, text)?,
            other => {
                return Err(format!(
                    "no view `{other}`: use cell, symbol, schematic, spice, digital, footprint, model3d or board"
                ));
            }
        }
    }
    Ok(symbol)
}

/// Where a sheet's port flags go when a cell is made with a schematic: inputs down the
/// left, outputs and the rest down the right.
fn place_ports(def: &mut CellDef) {
    let sheet = def.schematic.get_or_insert_with(Schematic::default);
    let (mut left, mut right) = (0.0, 0.0);
    for port in &def.cell.ports {
        if sheet.ports.iter().any(|p| p.port == port.name) {
            continue;
        }
        let input = matches!(port.kind, PortKind::Input | PortKind::PowerIn);
        let row = if input { &mut left } else { &mut right };
        let x = if input { -25.4 } else { 25.4 };
        sheet.ports.push(SchematicPort { port: port.name.clone(), at: Point::mm(x, *row) });
        *row += 5.08;
    }
}

fn summary(id: &CellId, def: &CellDef) -> String {
    let mut out = part_line(&id.to_string(), def);
    out.push('\n');
    if let Some(symbol) = &def.symbol {
        let pins: Vec<String> = symbol
            .pins
            .iter()
            .map(|p| format!("{} {} {}", p.port, at(p.at), outward_word(p.direction)))
            .collect();
        if !pins.is_empty() {
            let _ = writeln!(out, "Symbol pins (relative to its origin): {}", pins.join(", "));
        }
    }
    out
}

fn create_component(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let name = text(a, "name")?;
    validate_name(name)?;
    let lib = session.lib();
    if session.design.library(&lib).and_then(|l| l.cell(name)).is_some() {
        return Err(format!("`{name}` already exists: update_component changes it"));
    }
    let folder = interface::clean_folder(opt(a, "folder").unwrap_or(""));
    circuiter_model::validate_folder(&folder)?;
    let mut def = CellDef {
        name: name.to_owned(),
        folder,
        cell: Cell {
            description: opt(a, "description").unwrap_or("").to_owned(),
            prefix: "U".to_owned(),
            ports: parse_ports(items(a, "ports"))?,
            params: parse_params(items(a, "params"))?,
            ..Cell::default()
        },
        ..CellDef::default()
    };
    if let Some(prefix) = opt(a, "prefix") {
        interface::set_prefix(&mut def, prefix)?;
    }
    let drawn = apply_views(&mut def, a.get("views"))?;
    if !drawn && def.symbol.is_none() && !def.cell.ports.is_empty() {
        def.symbol = Some(builder::generate(name, &def.cell));
    }
    if flag(a, "schematic") {
        place_ports(&mut def);
    }
    let id = CellId::new(&lib, name);
    session.design.library_mut(&lib).expect("exists").insert(def);
    session.save()?;
    let mut out = format!("Made {id}.\n");
    out.push_str(&summary(&id, session.def(&id)));
    let problems = session.problems(&id);
    if !problems.is_empty() {
        let _ = write!(out, "Problems to fix:\n{problems}");
    }
    Ok(out)
}

fn update_component(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.find(text(a, "cell")?)?;
    let symbol_given =
        a.get("views").and_then(Value::as_object).is_some_and(|v| v.contains_key("symbol"));
    session.edit(&id, !symbol_given, |_, _, def| {
        if let Some(d) = opt(a, "description") {
            def.cell.description = d.to_owned();
        }
        if let Some(p) = opt(a, "prefix") {
            interface::set_prefix(def, p)?;
        }
        if let Some(f) = opt(a, "folder") {
            let folder = interface::clean_folder(f);
            circuiter_model::validate_folder(&folder)?;
            def.folder = folder;
        }
        if a.get("ports").is_some() {
            def.cell.ports = parse_ports(items(a, "ports"))?;
        }
        if a.get("params").is_some() {
            def.cell.params = parse_params(items(a, "params"))?;
        }
        apply_views(def, a.get("views"))?;
        if flag(a, "schematic") {
            place_ports(def);
        }
        Ok(())
    })?;
    if flag(a, "regenerate_symbol") {
        session.edit(&id, false, |_, _, def| {
            def.symbol =
                (!def.cell.ports.is_empty()).then(|| builder::generate(&def.name, &def.cell));
            Ok(())
        })?;
    }
    session.save()?;
    let mut out = format!("Updated {id}.\n");
    out.push_str(&summary(&id, session.def(&id)));
    let problems = session.problems(&id);
    if !problems.is_empty() {
        let _ = write!(out, "Problems to fix:\n{problems}");
    }
    Ok(out)
}

fn delete_cell(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.find(text(a, "cell")?)?;
    session.editable(&id)?;
    if id == session.top() {
        return Err(format!("{id} is the top cell: set_top another first"));
    }
    let mut users = Vec::new();
    for lib in session.design.libraries().filter(|l| l.name != STD_LIBRARY) {
        for def in lib.cells.values() {
            let uses = def.schematic.iter().flat_map(|s| &s.instances).any(|inst| {
                session.design.resolve(&inst.cell, &lib.name).is_some_and(|(c, _)| c == id)
            });
            if uses {
                users.push(format!("{}:{}", lib.name, def.name));
            }
        }
    }
    if !users.is_empty() && !flag(a, "force") {
        return Err(format!(
            "{id} is placed in {}: delete those parts first, or pass force",
            users.join(", ")
        ));
    }
    let def = session.design.library_mut(&id.library).expect("exists").cells.remove(&id.cell);
    session.save()?;
    let p = &session.project;
    let dir = match p.manifest.libraries.get(&id.library) {
        Some(entry) => p.root.join(&entry.path),
        None => p.root.clone(),
    };
    if let Some(def) = def {
        Library::remove_cell_files(&dir, &def).map_err(|e| e.to_string())?;
    }
    Ok(format!("Deleted {id}."))
}

fn set_top(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.find(text(a, "cell")?)?;
    if id.library != session.lib() {
        return Err("the top cell must be one of the project's own".into());
    }
    session.project.manifest.project.top = id.cell.clone();
    session.save()?;
    Ok(format!("{id} is now the top cell."))
}

fn import_spice(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let defs = circuiter_sim::parse::definitions(text(a, "text")?);
    if defs.is_empty() {
        return Err("no .subckt or .model card found".into());
    }
    let lib = session.lib();
    let mut taken: Vec<String> =
        session.design.library(&lib).map_or(Vec::new(), |l| l.cells.keys().cloned().collect());
    let mut out = String::new();
    for d in &defs {
        let name = crate::import::definition_name(d).to_owned();
        match crate::import::import(&session.design, d, &taken) {
            Ok(imported) => {
                let cell = imported.def.name.clone();
                let _ = write!(
                    out,
                    "- {name} ({}) → {lib}:{cell}. Ports: {}",
                    crate::import::describe(d),
                    ports_text(&imported.def.cell)
                );
                match &imported.warning {
                    Some(w) => {
                        let _ = writeln!(out, ". It won't simulate yet: {w}");
                    }
                    None => out.push_str(".\n"),
                }
                taken.push(cell);
                session.design.library_mut(&lib).expect("exists").insert(imported.def);
            }
            Err(e) => {
                let _ = writeln!(out, "- {name}: not imported: {e}");
            }
        }
    }
    session.save()?;
    Ok(out)
}

// Schematics.

/// Which way a wire leaves a pin whose body lies in `direction`.
fn outward_word(direction: PinDirection) -> &'static str {
    match direction {
        PinDirection::Right => "left",
        PinDirection::Left => "right",
        PinDirection::Down => "up",
        PinDirection::Up => "down",
    }
}

fn step_word(step: (i64, i64)) -> &'static str {
    match step {
        (1, 0) => "right",
        (-1, 0) => "left",
        (0, 1) => "down",
        (0, -1) => "up",
        _ => "",
    }
}

/// Each pin of `inst` on the sheet: its port, where it is, and the unit step a wire
/// leaving it takes.
fn sheet_pins(design: &Design, library: &str, inst: &Instance) -> Vec<(String, Point, (i64, i64))> {
    let Some((_, def)) = design.resolve(&inst.cell, library) else { return Vec::new() };
    let Some(symbol) = &def.symbol else { return Vec::new() };
    symbol
        .pins
        .iter()
        .map(|pin| {
            let toward = match pin.direction {
                PinDirection::Right => Point::mm(1.0, 0.0),
                PinDirection::Left => Point::mm(-1.0, 0.0),
                PinDirection::Down => Point::mm(0.0, 1.0),
                PinDirection::Up => Point::mm(0.0, -1.0),
            };
            let a = inst.placement.apply(pin.at);
            let b = inst.placement.apply(pin.at + toward);
            let step = ((a.x.nm() - b.x.nm()).signum(), (a.y.nm() - b.y.nm()).signum());
            (pin.port.clone(), a, step)
        })
        .collect()
}

fn pins_line(design: &Design, library: &str, inst: &Instance) -> String {
    let pins: Vec<String> = sheet_pins(design, library, inst)
        .into_iter()
        .map(|(port, p, step)| format!("{port} {} {}", at(p), step_word(step)))
        .collect();
    pins.join(", ")
}

/// The box a placed part's symbol covers.
fn part_box(design: &Design, library: &str, inst: &Instance) -> Option<(Point, Point)> {
    let (_, def) = design.resolve(&inst.cell, library)?;
    let (lo, hi) = def.symbol.as_ref()?.bounds()?;
    let (a, b) = (inst.placement.apply(lo), inst.placement.apply(hi));
    Some((Point::new(a.x.min(b.x), a.y.min(b.y)), Point::new(a.x.max(b.x), a.y.max(b.y))))
}

/// The extent of everything on a sheet, if anything is on it.
fn sheet_box(design: &Design, library: &str, sheet: &Schematic) -> Option<(Point, Point)> {
    let mut points: Vec<Point> = Vec::new();
    for inst in &sheet.instances {
        if let Some((lo, hi)) = part_box(design, library, inst) {
            points.extend([lo, hi]);
        }
    }
    points.extend(sheet.wires.iter().flat_map(|w| w.points.iter().copied()));
    points.extend(sheet.labels.iter().map(|l| l.at));
    points.extend(sheet.ports.iter().map(|p| p.at));
    let first = *points.first()?;
    Some(points.iter().fold((first, first), |(lo, hi), p| {
        (Point::new(lo.x.min(p.x), lo.y.min(p.y)), Point::new(hi.x.max(p.x), hi.y.max(p.y)))
    }))
}

fn snap_up(v: f64) -> f64 {
    (v / 2.54).ceil() * 2.54
}

fn describe_schematic(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let id = session.id(a)?;
    let def = session.def(&id);
    let sheet = def.schematic.as_ref().ok_or_else(|| format!("{id} has no schematic"))?;
    let design = &session.design;
    let conn = Connectivity::analyze(design, &id.library, sheet);
    let mut out = format!("Schematic of {id}");
    if let Some((lo, hi)) = sheet_box(design, &id.library, sheet) {
        let _ = write!(out, ", spanning {} to {}", at(lo), at(hi));
    }
    out.push_str(".\n\nParts:\n");
    if sheet.instances.is_empty() {
        out.push_str("(none)\n");
    }
    for inst in &sheet.instances {
        let p = inst.placement;
        let _ = write!(out, "- {}: {} at {}", inst.name, inst.cell, at(p.position));
        if p.rotation != Rotation::R0 {
            let _ = write!(out, " rotated {}", rotation_degrees(p.rotation));
        }
        if p.mirror {
            out.push_str(" mirrored");
        }
        if !inst.params.is_empty() {
            let params: Vec<String> =
                inst.params.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            let _ = write!(out, "; {}", params.join(", "));
        }
        if design.resolve(&inst.cell, &id.library).is_none() {
            out.push_str(" (MISSING: no such part)");
        }
        out.push('\n');
        if flag(a, "pins") {
            let _ = writeln!(out, "  pins: {}", pins_line(design, &id.library, inst));
        }
    }
    if !sheet.ports.is_empty() {
        out.push_str("\nSheet ports:\n");
        for port in &sheet.ports {
            let kind =
                def.cell.port(&port.port).map_or("not in the interface", |p| kind_name(p.kind));
            let _ = writeln!(out, "- {} ({kind}) at {}", port.port, at(port.at));
        }
    }
    out.push_str("\nNets:\n");
    let pin_name = |k: usize| {
        let pin = &conn.pins[k];
        format!("{}.{}", sheet.instances[pin.instance].name, pin.port)
    };
    let mut unnamed = 0;
    for net in &conn.nets {
        let members: Vec<String> = net
            .items
            .iter()
            .filter_map(|item| match *item {
                Item::Pin(k) => Some(pin_name(k)),
                Item::Port(i) => Some(format!("port {}", sheet.ports[i].port)),
                _ => None,
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        let name = if net.name.is_empty() {
            unnamed += 1;
            format!("(unnamed {unnamed})")
        } else {
            net.name.clone()
        };
        let global = if net.source == NetNameSource::Global { " [global]" } else { "" };
        let _ = writeln!(out, "- {name}{global}: {}", members.join(", "));
        if net.names.len() > 1 {
            let _ = writeln!(out, "  WARNING: named both {}", net.names.join(" and "));
        }
    }
    let open: Vec<String> = conn.dangling_pins.iter().map(|&k| pin_name(k)).collect();
    if !open.is_empty() {
        let _ = writeln!(out, "\nUnconnected pins: {}", open.join(", "));
    }
    for &(k, w) in &conn.passes_over {
        let _ = writeln!(
            out,
            "WARNING: wire {w} runs over pin {} without connecting to it",
            pin_name(k)
        );
    }
    if !conn.dangling_wire_ends.is_empty() {
        let ends: Vec<String> = conn.dangling_wire_ends.iter().map(|&p| at(p)).collect();
        let _ = writeln!(out, "WARNING: wire ends connected to nothing at {}", ends.join(", "));
    }
    if !sheet.notes.is_empty() || sheet.frame.is_some() {
        let _ = writeln!(
            out,
            "\n{} note(s){}.",
            sheet.notes.len(),
            if sheet.frame.is_some() { ", and a frame" } else { "" }
        );
    }
    Ok(out)
}

fn rotation_degrees(r: Rotation) -> u32 {
    match r {
        Rotation::R0 => 0,
        Rotation::R90 => 90,
        Rotation::R180 => 180,
        Rotation::R270 => 270,
    }
}

fn place_parts(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let parts = items(a, "parts");
    if parts.is_empty() {
        return Err("`parts` lists the parts to place".into());
    }
    let placed = session.edit(&id, true, |design, lib, def| {
        let sheet = def.schematic.get_or_insert_with(Schematic::default);
        // Parts without a position go in a row to the right of everything.
        let mut cursor =
            sheet_box(design, lib, sheet).map_or(0.0, |(_, hi)| snap_up(hi.x.mm() + 12.7));
        let row = sheet_box(design, lib, sheet).map_or(0.0, |(lo, _)| snap_up(lo.y.mm()));
        let mut placed = Vec::new();
        for spec in parts {
            let part = text(spec, "part")?;
            let cell: CellRef = part.parse()?;
            let (child, child_def) = design.resolve(&cell, lib).ok_or_else(|| {
                format!("no part `{part}`: search_parts finds one, create_component makes one")
            })?;
            let here = CellId::new(lib, &def.name);
            if design.contains(&child, &here) {
                return Err(format!("{part} contains {here}: placing it here would be recursive"));
            }
            if child_def.symbol.is_none() {
                return Err(format!(
                    "{part} has no symbol to place: give it ports, or a symbol view"
                ));
            }
            let name = match opt(spec, "name") {
                Some(n) => {
                    validate_name(n).map_err(|e| format!("reference `{n}`: {e}"))?;
                    if sheet.instance(n).is_some() {
                        return Err(format!("there is already a {n} on this sheet"));
                    }
                    n.to_owned()
                }
                None => edit::next_instance_name(sheet, &child_def.cell.prefix),
            };
            let mut params = BTreeMap::new();
            if let Some(given) = spec.get("params").and_then(Value::as_object) {
                for (k, v) in given {
                    let value = v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string());
                    let p = child_def.cell.param(k).ok_or_else(|| {
                        let known: Vec<&str> =
                            child_def.cell.params.iter().map(|p| p.name.as_str()).collect();
                        format!("{part} has no parameter `{k}` (it has: {})", known.join(", "))
                    })?;
                    p.kind.validate(&value).map_err(|e| format!("{name}.{k}: {e}"))?;
                    params.insert(k.clone(), value);
                }
            }
            let mut placement = Transform::default();
            if let Some(r) = spec.get("rotation") {
                placement.rotation = rotation(r)?;
            }
            placement.mirror = flag(spec, "mirror");
            let mut inst = Instance {
                name: name.clone(),
                cell: CellRef::new(Some(&child.library), &child.cell),
                placement,
                params,
                view: None,
                fields: BTreeMap::new(),
            };
            // A standard part is named in full; a part of the same library may be too.
            if child.library == lib {
                inst.cell = CellRef::new(None, &child.cell);
            }
            inst.placement.position = match spec.get("at") {
                Some(p) => edit::snap(to_point(p)?, GRID),
                None => {
                    let (lo, hi) = part_box(design, lib, &inst).unwrap_or_default();
                    let at = edit::snap(Point::mm(cursor - lo.x.mm(), row - lo.y.mm()), GRID);
                    cursor = snap_up(cursor + (hi.x - lo.x).mm() + 7.62);
                    at
                }
            };
            sheet.instances.push(inst.clone());
            placed.push(inst);
        }
        Ok(placed)
    })?;
    session.save()?;
    let mut out = format!("Placed on {id}:\n");
    for inst in &placed {
        let _ = writeln!(
            out,
            "- {} ({}) at {}. Pins: {}",
            inst.name,
            inst.cell,
            at(inst.placement.position),
            pins_line(&session.design, &id.library, inst)
        );
    }
    Ok(out)
}

/// A pin named in a tool's arguments: `R1.a`, or `port:IN` for a sheet port.
enum Target {
    Pin { instance: usize, port: String, at: Point, step: (i64, i64) },
    Port { index: usize, at: Point },
}

fn find_pin(design: &Design, lib: &str, def: &CellDef, name: &str) -> Result<Target, String> {
    let sheet = def.schematic.as_ref().ok_or("the cell has no schematic: place_parts first")?;
    if let Some(port) = name.strip_prefix("port:") {
        let index = sheet.ports.iter().position(|p| p.port == port).ok_or_else(|| {
            match def.cell.port(port) {
                Some(_) => format!("port {port} isn't placed on the sheet: place it with draw"),
                None => format!(
                    "{} has no port {port}: add it with update_component (and `schematic: true` places it)",
                    def.name
                ),
            }
        })?;
        return Ok(Target::Port { index, at: sheet.ports[index].at });
    }
    let (part, port) = name
        .split_once('.')
        .ok_or_else(|| format!("`{name}`: name a pin as part.port, like R1.a"))?;
    let instance = sheet
        .instances
        .iter()
        .position(|i| i.name == part)
        .ok_or_else(|| format!("no part {part} on the sheet"))?;
    let pins = sheet_pins(design, lib, &sheet.instances[instance]);
    let (_, at, step) = pins.iter().find(|(p, _, _)| p == port).cloned().ok_or_else(|| {
        let ports: Vec<&str> = pins.iter().map(|(p, _, _)| p.as_str()).collect();
        format!("{part} has no pin {port} (it has: {})", ports.join(", "))
    })?;
    Ok(Target::Pin { instance, port: port.to_owned(), at, step })
}

/// Whether nothing on the sheet sits at `p`: no pin, wire, label or port.
fn free(conn: &Connectivity, sheet: &Schematic, p: Point) -> bool {
    let on_wire = sheet.wires.iter().any(|w| {
        w.points.windows(2).any(|s| {
            let (a, b) = (s[0], s[1]);
            let (lo_x, hi_x) = (a.x.min(b.x), a.x.max(b.x));
            let (lo_y, hi_y) = (a.y.min(b.y), a.y.max(b.y));
            let inside = p.x >= lo_x && p.x <= hi_x && p.y >= lo_y && p.y <= hi_y;
            let cross = (b.x.nm() - a.x.nm()) as i128 * (p.y.nm() - a.y.nm()) as i128
                - (b.y.nm() - a.y.nm()) as i128 * (p.x.nm() - a.x.nm()) as i128;
            inside && cross == 0
        }) || w.points.contains(&p)
    });
    !on_wire
        && !conn.pins.iter().any(|q| q.at == p)
        && !sheet.labels.iter().any(|l| l.at == p)
        && !sheet.ports.iter().any(|q| q.at == p)
}

/// Whether `net` is a supply or ground net: one some part makes global (`GND`), or
/// named like a rail.
fn is_supply(design: &Design, net: &str) -> bool {
    let upper = net.to_ascii_uppercase();
    let named = upper == "GND"
        || upper == "VCC"
        || upper == "VDD"
        || upper == "VEE"
        || upper == "VSS"
        || upper.starts_with('+')
        || upper.starts_with('-');
    named
        || design
            .libraries()
            .flat_map(|l| l.cells.values())
            .any(|d| d.cell.global_net.as_deref() == Some(net))
}

/// Puts `target` on net `net`: a stub and a label, or a label on the pin itself where the
/// stub's end is taken.
fn attach(
    design: &Design,
    lib: &str,
    def: &mut CellDef,
    target: &Target,
    net: &str,
    global: bool,
) -> Result<(), String> {
    let sheet = def.schematic.as_ref().expect("checked");
    let conn = Connectivity::analyze(design, lib, sheet);
    let (at, step) = match *target {
        Target::Pin { at, step, .. } => (at, step),
        Target::Port { at, .. } => (at, (0, 0)),
    };
    let end = Point::mm(at.x.mm() + 2.54 * step.0 as f64, at.y.mm() + 2.54 * step.1 as f64);
    if step != (0, 0) && free(&conn, sheet, end) {
        edit::add_wire(def, vec![at, end]);
        edit::add_label(def, net, end, global)?;
    } else {
        edit::add_label(def, net, at, global)?;
    }
    Ok(())
}

/// How a pin comes off its net.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lift {
    /// Its part moves: wires drawn to the pin stay, named where the pin was, so the pin
    /// can rejoin them by label wherever it goes.
    Move,
    /// Its part goes: a lone wire to the pin goes too; wires meeting there stay joined.
    Delete,
    /// The pin leaves its net: every wire to it goes, and the rest of the net stays
    /// joined by name.
    Disconnect,
}

/// The pin or sheet port a wire end belongs to.
#[derive(Debug, Clone, Copy)]
enum Who<'a> {
    Pin(usize, &'a str),
    Port(usize),
}

/// Takes the pin (or port) `who` at `at` off its net, as `how` says. The short wires and
/// labels connect gave it go; what happens to other wires drawn to it depends on `how`.
/// Returns the nets to put it back on, each with whether it is global.
fn lift_pin(
    design: &Design,
    lib: &str,
    def: &mut CellDef,
    who: Who<'_>,
    at: Point,
    how: Lift,
) -> Vec<(String, bool)> {
    let Some(sheet) = def.schematic.as_ref() else { return Vec::new() };
    let conn = Connectivity::analyze(design, lib, sheet);
    let item = match who {
        Who::Pin(instance, port) => {
            conn.pins.iter().position(|p| p.instance == instance && p.port == port).map(Item::Pin)
        }
        Who::Port(index) => Some(Item::Port(index)),
    };
    let net = item.and_then(|i| conn.net_of(i)).map(|n| &conn.nets[n]);
    let labels_at =
        |p: Point| sheet.labels.iter().enumerate().filter(move |(_, l)| l.at == p).map(|(i, _)| i);
    let (mut stubs, mut wires, mut far_ends) = (BTreeSet::new(), Vec::new(), Vec::new());
    let mut labels: BTreeSet<usize> = labels_at(at).collect();
    for (w, wire) in sheet.wires.iter().enumerate() {
        let (first, last) = (wire.points[0], *wire.points.last().expect("points"));
        let far = if first == at {
            last
        } else if last == at {
            first
        } else {
            continue;
        };
        // A stub: nothing at its far end but labels.
        let others = conn.pins.iter().any(|p| p.at == far)
            || sheet.wires.iter().enumerate().any(|(v, o)| v != w && o.points.contains(&far))
            || sheet.ports.iter().any(|p| p.at == far);
        if others {
            wires.push(w);
            far_ends.push(far);
        } else {
            stubs.insert(w);
            labels.extend(labels_at(far));
        }
    }
    let mut nets: Vec<(String, bool)> = Vec::new();
    for &i in &labels {
        let entry = (sheet.labels[i].net.clone(), sheet.labels[i].global);
        if !nets.contains(&entry) {
            nets.push(entry);
        }
    }
    let mut add = Vec::new();
    let mut gone = stubs;
    if !wires.is_empty() {
        let (name, global) = match net {
            Some(n) if !n.name.is_empty() && n.source != NetNameSource::Auto => {
                (n.name.clone(), n.source == NetNameSource::Global)
            }
            _ => {
                let base = match who {
                    Who::Pin(instance, port) => {
                        format!("{}_{port}", sheet.instances[instance].name)
                    }
                    Who::Port(index) => sheet.ports[index].port.clone(),
                };
                let base: String =
                    base.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                let taken = |n: &str| {
                    sheet.labels.iter().any(|l| l.net == n) || conn.nets.iter().any(|m| m.name == n)
                };
                let mut name = base.clone();
                let mut k = 2;
                while taken(&name) {
                    name = format!("{base}_{k}");
                    k += 1;
                }
                (name, false)
            }
        };
        match how {
            Lift::Move => {
                add.push((name.clone(), at, global));
                if !nets.iter().any(|(n, _)| *n == name) {
                    nets.push((name, global));
                }
            }
            Lift::Delete => {
                if let [only] = wires[..] {
                    gone.insert(only);
                }
            }
            Lift::Disconnect => {
                if wires.len() > 1 {
                    add.extend(far_ends.iter().map(|&far| (name.clone(), far, global)));
                }
                gone.extend(wires);
            }
        }
    }
    let sheet = def.schematic.as_mut().expect("sheet");
    for &w in gone.iter().rev() {
        sheet.wires.remove(w);
    }
    for &i in labels.iter().rev() {
        sheet.labels.remove(i);
    }
    for (net, p, global) in add {
        let _ = edit::add_label(def, &net, p, global);
    }
    nets
}

fn connect(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let net = text(a, "net")?.trim().to_owned();
    if net.is_empty() || net.chars().any(char::is_whitespace) {
        return Err("give the net a name without spaces, such as VOUT".into());
    }
    let pins = names(a, "pins")?;
    if pins.is_empty() {
        return Err("`pins` lists the pins to join".into());
    }
    let global = flag(a, "global") || is_supply(&session.design, &net);
    let (made, already) = session.edit(&id, true, |design, lib, def| {
        let (mut made, mut already) = (Vec::new(), Vec::new());
        for pin in &pins {
            let target = find_pin(design, lib, def, pin)?;
            let sheet = def.schematic.as_ref().expect("found");
            let conn = Connectivity::analyze(design, lib, sheet);
            let item = match target {
                Target::Pin { instance, ref port, .. } => conn
                    .pins
                    .iter()
                    .position(|p| p.instance == instance && p.port == *port)
                    .map(Item::Pin),
                Target::Port { index, .. } => Some(Item::Port(index)),
            };
            if let Some(n) = item.and_then(|i| conn.net_of(i)) {
                let on = &conn.nets[n];
                if on.names.contains(&net) {
                    already.push(pin.to_string());
                    continue;
                }
                let named = !on.name.is_empty() && on.source != NetNameSource::Auto;
                if named && on.source != NetNameSource::Port {
                    return Err(format!(
                        "{pin} is already on net {}: disconnect it first, or connect to {}",
                        on.name, on.name
                    ));
                }
            }
            attach(design, lib, def, &target, &net, global)?;
            made.push(pin.to_string());
        }
        Ok((made, already))
    })?;
    session.save()?;
    let mut out = format!("Net {net}{}: ", if global { " (global)" } else { "" });
    if made.is_empty() {
        out.push_str("nothing to do");
    } else {
        let _ = write!(out, "connected {}", made.join(", "));
    }
    if !already.is_empty() {
        let _ = write!(out, "; already on it: {}", already.join(", "));
    }
    out.push('.');
    Ok(out)
}

fn disconnect(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let pins = names(a, "pins")?;
    let mut out = String::new();
    session.edit(&id, true, |design, lib, def| {
        for pin in &pins {
            let nets = match find_pin(design, lib, def, pin)? {
                Target::Pin { instance, port, at, .. } => {
                    lift_pin(design, lib, def, Who::Pin(instance, &port), at, Lift::Disconnect)
                }
                Target::Port { index, at } => {
                    lift_pin(design, lib, def, Who::Port(index), at, Lift::Disconnect)
                }
            };
            let names: Vec<&str> = nets.iter().map(|(n, _)| n.as_str()).collect();
            let _ = writeln!(
                out,
                "- {pin}: {}",
                if names.is_empty() {
                    "off its wires".into()
                } else {
                    format!("off {}", names.join(", "))
                }
            );
        }
        Ok(())
    })?;
    session.save()?;
    Ok(out)
}

/// Takes every pin of part `instance` off its nets, as `how` says: the nets to put each
/// pin back on, by port.
fn lift(
    design: &Design,
    lib: &str,
    def: &mut CellDef,
    instance: usize,
    how: Lift,
) -> Vec<(String, String, bool)> {
    let inst = def.schematic.as_ref().expect("sheet").instances[instance].clone();
    let mut nets = Vec::new();
    for (port, at, _) in sheet_pins(design, lib, &inst) {
        for (net, global) in lift_pin(design, lib, def, Who::Pin(instance, &port), at, how) {
            nets.push((port.clone(), net, global));
        }
    }
    nets
}

fn land(
    design: &Design,
    lib: &str,
    def: &mut CellDef,
    instance: usize,
    nets: &[(String, String, bool)],
) -> Result<(), String> {
    let inst = def.schematic.as_ref().expect("sheet").instances[instance].clone();
    let pins = sheet_pins(design, lib, &inst);
    for (port, net, global) in nets {
        if let Some((_, at, step)) = pins.iter().find(|(p, _, _)| p == port) {
            let target = Target::Pin { instance, port: port.clone(), at: *at, step: *step };
            attach(design, lib, def, &target, net, *global)?;
        }
    }
    Ok(())
}

fn edit_parts(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let mut out = String::new();
    session.edit(&id, true, |design, lib, def| {
        for spec in items(a, "parts") {
            let name = text(spec, "name")?;
            let sheet = def.schematic.as_ref().ok_or("the cell has no schematic")?;
            let k = sheet
                .instances
                .iter()
                .position(|i| i.name == name)
                .ok_or_else(|| format!("no part {name} on the sheet"))?;
            let child = design
                .resolve(&sheet.instances[k].cell, lib)
                .map(|(_, d)| d.clone())
                .ok_or_else(|| format!("{name}'s part is missing"))?;
            if let Some(params) = spec.get("params").and_then(Value::as_object) {
                for (p, v) in params {
                    let def_p = child
                        .cell
                        .param(p)
                        .ok_or_else(|| format!("{name} has no parameter `{p}`"))?;
                    let value = match v {
                        Value::Null => None,
                        Value::String(t) => Some(t.clone()),
                        other => Some(other.to_string()),
                    };
                    if let Some(value) = &value {
                        def_p.kind.validate(value).map_err(|e| format!("{name}.{p}: {e}"))?;
                    }
                    edit::set_param(def, k, p, value);
                }
            }
            let moves = spec.get("at").is_some()
                || spec.get("rotation").is_some()
                || spec.get("mirror").is_some();
            if moves {
                let nets = lift(design, lib, def, k, Lift::Move);
                let inst = &mut def.schematic.as_mut().expect("sheet").instances[k];
                if let Some(p) = spec.get("at") {
                    inst.placement.position = edit::snap(to_point(p)?, GRID);
                }
                if let Some(r) = spec.get("rotation") {
                    inst.placement.rotation = rotation(r)?;
                }
                if let Some(m) = spec.get("mirror").and_then(Value::as_bool) {
                    inst.placement.mirror = m;
                }
                land(design, lib, def, k, &nets)?;
            }
            let mut now = name.to_owned();
            if let Some(new) = opt(spec, "rename") {
                edit::rename(def, k, new)?;
                now = new.to_owned();
            }
            let inst = &def.schematic.as_ref().expect("sheet").instances[k];
            let _ = writeln!(
                out,
                "- {now} at {}. Pins: {}",
                at(inst.placement.position),
                pins_line(design, lib, inst)
            );
        }
        Ok(())
    })?;
    session.save()?;
    Ok(out)
}

fn delete_parts(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let gone = names(a, "names")?;
    session.edit(&id, true, |design, lib, def| {
        for name in &gone {
            let sheet = def.schematic.as_ref().ok_or("the cell has no schematic")?;
            let k = sheet
                .instances
                .iter()
                .position(|i| i.name == *name)
                .ok_or_else(|| format!("no part {name} on the sheet"))?;
            let pins: Vec<Point> =
                sheet_pins(design, lib, &sheet.instances[k]).into_iter().map(|p| p.1).collect();
            lift(design, lib, def, k, Lift::Delete);
            let sheet = def.schematic.as_mut().expect("sheet");
            sheet.no_connects.retain(|p| !pins.contains(p));
            sheet.instances.remove(k);
        }
        Ok(())
    })?;
    session.save()?;
    Ok(format!("Removed {} from {id}.", gone.join(", ")))
}

fn draw(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let mut done = Vec::new();
    session.edit(&id, true, |_, _, def| {
        def.schematic.get_or_insert_with(Schematic::default);
        let wires = items(a, "wires");
        for wire in wires {
            let points: Vec<Point> = wire
                .as_array()
                .ok_or("each wire is a list of points")?
                .iter()
                .map(|p| to_point(p).map(|p| edit::snap(p, GRID)))
                .collect::<Result<_, _>>()?;
            edit::add_wire(def, points);
        }
        if !wires.is_empty() {
            done.push(format!("{} wire(s)", wires.len()));
        }
        let labels = items(a, "labels");
        for l in labels {
            edit::add_label(
                def,
                text(l, "net")?,
                to_point(l.get("at").unwrap_or(&Value::Null))?,
                flag(l, "global"),
            )?;
        }
        if !labels.is_empty() {
            done.push(format!("{} label(s)", labels.len()));
        }
        for p in items(a, "ports") {
            let name = text(p, "name")?;
            if def.cell.port(name).is_none() {
                return Err(format!(
                    "{} has no port {name}: add it with update_component first",
                    def.name
                ));
            }
            let place = to_point(p.get("at").unwrap_or(&Value::Null))?;
            let sheet = def.schematic.as_mut().expect("made");
            sheet.ports.retain(|q| q.port != name);
            sheet.ports.push(SchematicPort { port: name.to_owned(), at: place });
            done.push(format!("port {name}"));
        }
        for p in items(a, "no_connects") {
            let p = to_point(p)?;
            let sheet = def.schematic.as_mut().expect("made");
            if !sheet.no_connects.contains(&p) {
                sheet.no_connects.push(p);
            }
            done.push(format!("no-connect at {}", at(p)));
        }
        for n in items(a, "notes") {
            let note = match (n.get("from"), n.get("to")) {
                (Some(from), Some(to)) => Note::Box {
                    a: to_point(from)?,
                    b: to_point(to)?,
                    title: opt(n, "title").unwrap_or("").to_owned(),
                },
                _ => Note::Text {
                    at: to_point(n.get("at").ok_or("a text note needs `at`")?)?,
                    text: text(n, "text")?.to_owned(),
                },
            };
            edit::add_note(def, note)?;
            done.push("a note".into());
        }
        Ok(())
    })?;
    session.save()?;
    if done.is_empty() {
        return Ok("Nothing to draw.".into());
    }
    Ok(format!("Drew {} on {id}.", done.join(", ")))
}

fn arrange_sheet(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let (parts, wires) = session.edit(&id, true, |design, lib, def| {
        let sheet = def.schematic.as_ref().ok_or("the cell has no schematic")?;
        let laid = circuiter_diagram::arrange_sheet(design, lib, &def.cell, sheet);
        edit::use_layout(design, lib, def, laid);
        let sheet = def.schematic.as_ref().expect("laid out");
        Ok((sheet.instances.len(), sheet.wires.len()))
    })?;
    session.save()?;
    Ok(format!(
        "Laid out {id}: {parts} symbols and {wires} wires. describe_schematic shows the result."
    ))
}

fn set_frame(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    let remove = flag(a, "remove");
    let (paper, portrait) = session.edit(&id, true, |design, _, def| {
        let sheet = def.schematic.as_mut().ok_or("the cell has no schematic")?;
        if remove {
            sheet.frame = None;
            return Ok((None, false));
        }
        let mut frame = sheet.frame.take().unwrap_or_else(|| Frame::new(Paper::A4));
        let fields: [(&str, &mut String); 5] = [
            ("title", &mut frame.title),
            ("revision", &mut frame.revision),
            ("date", &mut frame.date),
            ("company", &mut frame.company),
            ("comment", &mut frame.comment),
        ];
        for (key, field) in fields {
            if let Some(v) = opt(a, key) {
                *field = v.to_owned();
            }
        }
        let bounds = circuiter_scene::Scene::build_from(design, &id, &def.cell, sheet).bounds;
        if !bounds.is_empty() {
            edit::fit_frame(
                &mut frame,
                Point::mm(bounds.min.x, bounds.min.y),
                Point::mm(bounds.max.x, bounds.max.y),
            );
        }
        let out = (Some(frame.paper), frame.portrait);
        sheet.frame = Some(frame);
        Ok(out)
    })?;
    session.save()?;
    Ok(match paper {
        None => format!("Took the frame off {id}."),
        Some(p) => format!(
            "Framed {id} on {} {}.",
            p.name(),
            if portrait { "portrait" } else { "landscape" }
        ),
    })
}

// Checks and simulation.

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn check(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let id = session.id(a)?;
    let mut libs = vec![session.lib()];
    libs.extend(session.project.manifest.libraries.keys().cloned());
    let refs: Vec<&str> = libs.iter().map(String::as_str).collect();
    let found = check_libraries(&session.design, &refs);
    let mut out = String::new();
    let (mut errors, mut warnings) = (0, 0);
    let mut count = |s: Severity| match s {
        Severity::Error => errors += 1,
        Severity::Warning => warnings += 1,
    };
    if !found.is_empty() {
        out.push_str("Cells:\n");
        out.push_str(&diagnostics(&found));
        found.iter().for_each(|d| count(d.severity));
    }
    if session.def(&id).schematic.is_some() {
        let erc = circuiter_netlist::erc::check(&session.design, &id);
        if !erc.is_empty() {
            let _ = writeln!(out, "Electrical rules of {id}:");
        }
        for v in &erc {
            count(v.severity);
            let mut place = String::new();
            if let Some(sheet) = v.sheet.as_deref().filter(|s| !s.is_empty()) {
                let _ = write!(place, " in {sheet}");
            }
            if let Some(p) = v.at {
                let _ = write!(place, " at {}", at(p));
            }
            let _ = writeln!(out, "- {}{place}: {}", severity(v.severity), v.message);
        }
    }
    if out.is_empty() {
        return Ok(format!("No problems found in {id} or the project's cells."));
    }
    Ok(format!("{errors} error(s), {warnings} warning(s).\n{out}"))
}

fn simulate(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let root = s.root()?.to_path_buf();
    let id = session.id(a)?;
    if session.def(&id).schematic.is_none() {
        return Err(format!("{id} has no schematic to simulate"));
    }
    let mut sim = Simulation::prepare_in(&session.design, &id, &root)?;
    if sim.has_errors() {
        let errors: Vec<Diagnostic> =
            sim.diagnostics.iter().filter(|d| d.severity == Severity::Error).cloned().collect();
        return Err(format!("{id} can't be simulated yet:\n{}", diagnostics(&errors)));
    }
    let analysis = match text(a, "analysis")?.to_ascii_lowercase().as_str() {
        "op" | "dc" => Analysis::Op,
        "tran" | "transient" => {
            let (tstop, tstep) = simulation::default_tran(&sim.circuit);
            let tstop = quantity(a, "tstop")?.unwrap_or(tstop);
            let tstep = quantity(a, "tstep")?.unwrap_or(tstep.min(tstop / 100.0));
            Analysis::Tran { tstop, tstep, power_on: flag(a, "power_on") }
        }
        "ac" => Analysis::Ac {
            fstart: quantity(a, "fstart")?.unwrap_or(1.0),
            fstop: quantity(a, "fstop")?.unwrap_or(1e6),
            per_decade: a.get("per_decade").and_then(Value::as_u64).unwrap_or(20) as usize,
        },
        other => return Err(format!("analysis `{other}`: use op, tran or ac")),
    };
    sim.run(analysis);
    if let Some(e) = &sim.error {
        return Err(format!("the simulation failed: {e}"));
    }
    let nets = &sim.netlist.nets;
    let asked = names(a, "probes")?;
    let probes: Vec<usize> = if asked.is_empty() {
        simulation::default_probes(&sim.netlist)
    } else {
        asked
            .iter()
            .map(|name| {
                nets.iter().position(|n| n.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
                    let known: Vec<&str> = nets.iter().take(40).map(|n| n.name.as_str()).collect();
                    format!("no net {name} (nets: {})", known.join(", "))
                })
            })
            .collect::<Result<_, _>>()?
    };
    let mut out = String::new();
    let warnings: Vec<Diagnostic> =
        sim.diagnostics.iter().filter(|d| d.severity == Severity::Warning).cloned().collect();
    match &sim.results {
        Some(Results::Op(_)) => {
            let _ = writeln!(out, "Operating point of {id}.\nNet voltages:");
            for (k, net) in nets.iter().enumerate().skip(1) {
                if let Some(v) = sim.voltage(k) {
                    let _ = writeln!(out, "- {} = {}", net.name, simulation::show(v, "V"));
                }
            }
            let mut parts = Vec::new();
            for e in &sim.netlist.elements {
                let Some((currents, power)) = sim.element_op(&e.path) else { continue };
                let flows: Vec<String> = currents
                    .iter()
                    .map(|(port, i)| format!("{port} {}", simulation::show(*i, "A")))
                    .collect();
                parts.push(format!(
                    "- {}: {}; {}",
                    e.path,
                    simulation::show(power, "W"),
                    flows.join(", ")
                ));
            }
            if !parts.is_empty() {
                let _ = writeln!(out, "Parts (power absorbed; current into each terminal):");
                for p in parts.iter().take(80) {
                    let _ = writeln!(out, "{p}");
                }
            }
            for assessment in &sim.assessments {
                let _ = writeln!(out, "WARNING: {assessment:?}");
            }
        }
        Some(Results::Tran { tstop, .. } | Results::Mixed { tstop, .. }) => {
            let _ = writeln!(out, "Transient run of {id} to {}.", simulation::show(*tstop, "s"));
            for &k in &probes {
                let Some((time, values)) = sim.waveform(k) else { continue };
                waveform(&mut out, &nets[k].name, time, &values);
            }
            if sim.logic_run().is_some() {
                logic_report(&mut out, &sim, &probes);
            }
        }
        Some(Results::Logic(logic)) => {
            let _ = writeln!(
                out,
                "Logic run of {id} to {}{}.",
                simulation::show(logic.end, "s"),
                if logic.halted { " (halted)" } else { "" }
            );
            logic_report(&mut out, &sim, &probes);
        }
        Some(Results::Ac { .. }) => {
            let _ = writeln!(out, "AC sweep of {id}: gain (dB) and phase at each frequency.");
            let mut biggest: f64 = 0.0;
            for &k in &probes {
                let Some((freq, response)) = sim.response(k) else { continue };
                biggest = response.iter().fold(biggest, |m, h| m.max(h.re.hypot(h.im)));
                let _ = writeln!(out, "{}:", nets[k].name);
                let step = freq.len().div_ceil(24).max(1);
                for (i, (f, h)) in freq.iter().zip(&response).enumerate() {
                    if i % step == 0 || i + 1 == freq.len() {
                        let db = 20.0 * h.re.hypot(h.im).max(1e-30).log10();
                        let deg = h.im.atan2(h.re).to_degrees();
                        let _ = writeln!(
                            out,
                            "  {}: {} dB, {}°",
                            simulation::show(*f, "Hz"),
                            num(db),
                            num(deg)
                        );
                    }
                }
            }
            if biggest < 1e-20 {
                out.push_str(
                    "Nothing drives the circuit in this sweep: it needs a source with an AC \
                     amplitude, such as std:vsine (its `ac` parameter).\n",
                );
            }
        }
        None => return Err("the simulation produced nothing".into()),
    }
    for e in sim.events() {
        let _ = writeln!(out, "DESTROYED: {}", e.describe());
    }
    if !warnings.is_empty() {
        let _ = write!(out, "Warnings:\n{}", diagnostics(&warnings));
    }
    Ok(out)
}

fn waveform(out: &mut String, name: &str, time: &[f64], values: &[f64]) {
    if values.is_empty() {
        return;
    }
    let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let _ = writeln!(
        out,
        "{name}: from {} to {}, ending at {}. Samples (time: value):",
        simulation::show(lo, "V"),
        simulation::show(hi, "V"),
        simulation::show(*values.last().expect("values"), "V")
    );
    let n = 20;
    let end = *time.last().expect("time");
    let mut k = 0;
    let mut line = Vec::new();
    for step in 0..=n {
        let t = end * step as f64 / n as f64;
        while k + 1 < time.len() && time[k + 1] <= t {
            k += 1;
        }
        line.push(format!(
            "{} {}",
            simulation::show(time[k], "s"),
            simulation::show(values[k], "V")
        ));
    }
    let _ = writeln!(out, "  {}", line.join("; "));
}

fn logic_report(out: &mut String, sim: &Simulation, probes: &[usize]) {
    for &k in probes {
        let Some(history) = sim.logic(k) else { continue };
        let changes: Vec<String> = history
            .iter()
            .take(24)
            .map(|(t, v)| format!("{} {v}", simulation::show(*t as f64 * 1e-12, "s")))
            .collect();
        let last = history.last().map(|(_, v)| v.to_string()).unwrap_or_default();
        let _ = writeln!(
            out,
            "{}: ends {last}; changes: {}{}",
            sim.netlist.nets[k].name,
            changes.join(", "),
            if history.len() > 24 { ", …" } else { "" }
        );
    }
    if let Some(v) = sim.program_result() {
        let _ = writeln!(out, "Program result: {v}");
    }
}

// Boards.

fn board_of(session: &Session, id: &CellId) -> Result<circuiter_model::board::Board, String> {
    session
        .def(id)
        .board
        .clone()
        .ok_or_else(|| format!("{id} has no board: pcb_new_board makes one"))
}

fn pcb_new_board(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    session.editable(&id)?;
    if session.def(&id).schematic.is_none() {
        return Err(format!("{id} has no schematic to lay out"));
    }
    if session.def(&id).board.is_some() && !flag(a, "replace") {
        return Err(format!("{id} already has a board: pass replace to start again"));
    }
    let mut board = circuiter_pcb::sync::new_board(&session.design, &id)?;
    if let Some(n) = a.get("layers").and_then(Value::as_u64) {
        if !matches!(n, 2 | 4 | 6) {
            return Err("layers: 2, 4 or 6".into());
        }
        board.copper_layers = n as u8;
    }
    let (before, after) = circuiter_pcb::sync::optimize(&session.design, &id, &mut board)?;
    let parts = board.footprints.len();
    let (lo, hi) =
        board.outline.iter().fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(lo, hi), p| {
            ((lo.0.min(p.x.mm()), lo.1.min(p.y.mm())), (hi.0.max(p.x.mm()), hi.1.max(p.y.mm())))
        });
    session.design.cell_mut(&id).expect("exists").board = Some(board);
    session.save()?;
    Ok(format!(
        "Made a {}-layer board for {id}, {} × {} mm, with {parts} footprints placed; the \
         placement optimiser cut the connections from {} to {} mm. Next: pcb_autoroute, \
         then pcb_check.",
        session.def(&id).board.as_ref().map_or(2, |b| b.copper_layers),
        num(hi.0 - lo.0),
        num(hi.1 - lo.1),
        num(before),
        num(after)
    ))
}

fn pcb_update(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    session.editable(&id)?;
    let mut board = board_of(&session, &id)?;
    let changes = circuiter_pcb::sync::changes(&session.design, &id, &board)?;
    if changes.is_empty() {
        return Ok("The board already matches the schematic.".into());
    }
    circuiter_pcb::sync::apply(&session.design, &id, &mut board, &changes)?;
    session.design.cell_mut(&id).expect("exists").board = Some(board);
    session.save()?;
    let lines: Vec<String> = changes.iter().map(|c| format!("- {}", c.describe())).collect();
    Ok(format!("Updated the board of {id}:\n{}", lines.join("\n")))
}

fn pcb_place(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    session.editable(&id)?;
    let mut board = board_of(&session, &id)?;
    let mut moved = Vec::new();
    for spec in items(a, "parts") {
        let path = text(spec, "ref")?;
        let old = board
            .footprints
            .iter()
            .find(|f| f.path == path)
            .cloned()
            .ok_or_else(|| format!("{path} isn't on the board"))?;
        let layout = circuiter_pcb::layout::Layout::new(&session.design, &id, &board)?;
        let placed = Placed {
            path: path.to_owned(),
            at: to_point(spec.get("at").unwrap_or(&Value::Null))?,
            rotation: spec.get("rotation").and_then(Value::as_f64).unwrap_or(old.rotation),
            back: spec.get("back").and_then(Value::as_bool).unwrap_or(old.back),
            locked: spec.get("locked").and_then(Value::as_bool).unwrap_or(old.locked),
        };
        moved.push(format!("{path} to {}", at(placed.at)));
        circuiter_pcb::edit::move_footprint(&mut board, &layout, path, placed);
    }
    session.design.cell_mut(&id).expect("exists").board = Some(board);
    session.save()?;
    Ok(format!("Moved {}.", moved.join(", ")))
}

fn pcb_autoroute(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    session.editable(&id)?;
    let mut board = board_of(&session, &id)?;
    let layout = circuiter_pcb::layout::Layout::new(&session.design, &id, &board)?;
    let only = names(a, "nets")?;
    let opts = circuiter_pcb::route::Options {
        nets: (!only.is_empty()).then(|| only.iter().map(|n| n.to_string()).collect()),
        ..Default::default()
    };
    let routed = circuiter_pcb::route::autoroute(&layout, &opts);
    board.tracks.extend(routed.tracks);
    board.vias.extend(routed.vias);
    session.design.cell_mut(&id).expect("exists").board = Some(board);
    session.save()?;
    let mut out = format!("Routed {} connection(s).", routed.done);
    if !routed.failed.is_empty() {
        let nets: BTreeSet<&str> = routed.failed.iter().map(|f| f.0.as_str()).collect();
        let _ = write!(
            out,
            " {} couldn't be routed, on {}: move parts apart (pcb_place) or add layers, and \
             route again.",
            routed.failed.len(),
            nets.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(out)
}

fn pcb_pour(s: &mut Server, a: &Value) -> Result<String, String> {
    let mut session = s.session()?;
    let id = session.id(a)?;
    session.editable(&id)?;
    let mut board = board_of(&session, &id)?;
    let net = text(a, "net")?;
    let layout = circuiter_pcb::layout::Layout::new(&session.design, &id, &board)?;
    if !layout.nets.contains(net) {
        let known: Vec<&str> = layout.nets.iter().take(30).map(String::as_str).collect();
        return Err(format!("no net {net} on the board (nets: {})", known.join(", ")));
    }
    let copper = board.copper();
    let asked = names(a, "layers")?;
    let layers: Vec<_> = if asked.is_empty() {
        vec![copper[0], *copper.last().expect("copper")]
    } else {
        asked
            .iter()
            .map(|name| {
                copper.iter().copied().find(|l| l.name().eq_ignore_ascii_case(name)).ok_or_else(
                    || {
                        let all: Vec<String> = copper.iter().map(|l| l.name()).collect();
                        format!("no copper layer {name} (the board has {})", all.join(", "))
                    },
                )
            })
            .collect::<Result<_, _>>()?
    };
    for &layer in &layers {
        board.zones.retain(|z| !(z.layer == layer && z.net == net));
        board.zones.push(Zone::new(layer, net, board.outline.clone()));
    }
    session.design.cell_mut(&id).expect("exists").board = Some(board);
    session.save()?;
    let names: Vec<String> = layers.iter().map(|l| l.name()).collect();
    Ok(format!("Poured {net} on {}.", names.join(", ")))
}

fn pcb_check(s: &mut Server, a: &Value) -> Result<String, String> {
    let session = s.session()?;
    let id = session.id(a)?;
    let board = board_of(&session, &id)?;
    let layout = circuiter_pcb::layout::Layout::new(&session.design, &id, &board)?;
    let mut out = String::new();
    for e in &layout.errors {
        let _ = writeln!(out, "- error: {e}");
    }
    if !layout.unplaced.is_empty() {
        let _ =
            writeln!(out, "- not on the board yet (pcb_update): {}", layout.unplaced.join(", "));
    }
    if !layout.orphans.is_empty() {
        let _ =
            writeln!(out, "- on the board but not in the schematic: {}", layout.orphans.join(", "));
    }
    let checked = circuiter_pcb::drc::check(&layout);
    for v in checked.violations.iter().take(60) {
        let place = v.at.map(|p| format!(" at ({}, {})", num(p[0]), num(p[1]))).unwrap_or_default();
        let level = match v.severity() {
            circuiter_pcb::drc::Severity::Error => "error",
            circuiter_pcb::drc::Severity::Warning => "warning",
        };
        let _ = writeln!(out, "- {level}{place}: {}", v.message);
    }
    if checked.violations.len() > 60 {
        let _ = writeln!(out, "… and {} more", checked.violations.len() - 60);
    }
    for v in circuiter_pcb::drc::bodies(&session.design, &layout) {
        let _ = writeln!(out, "- 3D: {}", v.message);
    }
    let unrouted = checked.connectivity.unrouted();
    let head = format!(
        "{} rule violation(s), {unrouted} connection(s) unrouted, {} tracks and {} vias.",
        checked.errors(),
        board.tracks.len(),
        board.vias.len()
    );
    Ok(if out.is_empty() { format!("{head} All clear.") } else { format!("{head}\n{out}") })
}

// Files.

/// `rel` under the project, refusing anything that would leave it.
fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = Path::new(rel);
    if path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("`{rel}`: give a path inside the project, without `..`"));
    }
    Ok(root.join(path))
}

fn list_files(s: &mut Server, a: &Value) -> Result<String, String> {
    let root = s.root()?.to_path_buf();
    let dir = inside(&root, opt(a, "path").unwrap_or(""))?;
    let mut out = Vec::new();
    let mut stack = vec![(dir.clone(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d)
            .map_err(|e| format!("{}: {e}", d.display()))?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with('.') && name != "target"
            })
            .collect();
        entries.sort();
        for p in entries.into_iter().rev() {
            let rel = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
            if p.is_dir() {
                out.push(format!("{rel}/"));
                if depth < 3 {
                    stack.push((p, depth + 1));
                }
            } else {
                out.push(rel);
            }
        }
    }
    out.sort();
    if out.len() > 400 {
        let more = out.len() - 400;
        out.truncate(400);
        out.push(format!("… and {more} more"));
    }
    Ok(out.join("\n"))
}

fn read_file(s: &mut Server, a: &Value) -> Result<String, String> {
    let path = inside(s.root()?, text(a, "path")?)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() > 400_000 {
        return Err(format!("{} is too big to read here ({} bytes)", path.display(), text.len()));
    }
    Ok(text)
}

fn write_file(s: &mut Server, a: &Value) -> Result<String, String> {
    let root = s.root()?.to_path_buf();
    let rel = text(a, "path")?;
    let path = inside(&root, rel)?;
    let contents = text(a, "text")?;
    let before = std::fs::read_to_string(&path).ok();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    let design_file = rel == circuiter_model::project::MANIFEST_FILE
        || Path::new(rel).components().next().is_some_and(|c| c.as_os_str() == "cells");
    if design_file {
        if let Err(e) = s.session() {
            let _ = match before {
                Some(old) => std::fs::write(&path, old),
                None => std::fs::remove_file(&path),
            };
            return Err(format!("that breaks the project, so it was put back: {e}"));
        }
    }
    Ok(format!("Wrote {rel} ({} bytes).", contents.len()))
}
