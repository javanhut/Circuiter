//! The board editor, after KiCad's: a cell's printed circuit board drawn layer by layer;
//! parts placed, moved, turned, flipped and locked; tracks routed at 45° from pad to
//! pad with vias to change layer; copper pours; the ratsnest of what's left to route;
//! design rules checked as you go; and the files for the board house.

use super::CircuiterApp;
use crate::canvas::Camera;
use crate::style::{self, icon};
use circuiter_model::board::{Board, Graphic, Layer, PadConnection, Placed, Track, Via, Zone};
use circuiter_model::geom::Length;
use circuiter_model::{CellId, Design};
use circuiter_pcb::drc::{self, Checked, Severity};
use circuiter_pcb::geometry::{self, Contour};
use circuiter_pcb::layout::{Layout, P, PadAt, Place, p, point};
use circuiter_pcb::route::{Options as RouteOptions, Router};
use circuiter_pcb::{fab, font, sync};
use circuiter_scene::Pt;
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke};

/// Grid steps on offer, in millimetres.
pub const GRIDS: &[f64] = &[0.1, 0.25, 0.5, 0.635, 1.0, 1.27, 2.54];

const BACKGROUND: Color32 = Color32::from_rgb(11, 17, 26);
const BOARD_FILL: Color32 = Color32::from_rgb(23, 43, 46);
const EDGE: Color32 = Color32::from_rgb(149, 185, 176);
const SILK_FRONT: Color32 = Color32::from_rgb(222, 230, 213);
const SILK_BACK: Color32 = Color32::from_rgb(232, 178, 167);
const RATSNEST: Color32 = Color32::from_rgb(140, 166, 185);
const HOLE: Color32 = Color32::from_rgb(11, 17, 26);
const VIA: Color32 = Color32::from_rgb(194, 194, 194);
const SELECTED: Color32 = Color32::WHITE;
/// The realistic view's colours: green solder mask, copper seen through it, gold
/// (ENIG) pads and a white legend.
const MASK: Color32 = Color32::from_rgb(18, 82, 44);
const MASK_COPPER: Color32 = Color32::from_rgb(44, 128, 62);
const GOLD: Color32 = Color32::from_rgb(214, 176, 96);
const LEGEND: Color32 = Color32::from_rgb(240, 240, 232);

/// A copper layer's colour, as KiCad shows it.
pub fn layer_colour(layer: Layer) -> Color32 {
    match layer {
        Layer::FCu => Color32::from_rgb(239, 113, 103),
        Layer::BCu => Color32::from_rgb(101, 165, 235),
        Layer::In(1) => Color32::from_rgb(127, 200, 127),
        Layer::In(2) => Color32::from_rgb(206, 125, 44),
        Layer::In(3) => Color32::from_rgb(180, 110, 220),
        Layer::In(_) => Color32::from_rgb(80, 200, 200),
        Layer::FSilk => SILK_FRONT,
        Layer::BSilk => SILK_BACK,
        Layer::FCrtYd | Layer::BCrtYd => Color32::from_rgb(255, 38, 226),
        Layer::FFab | Layer::BFab => Color32::from_rgb(175, 175, 175),
        Layer::EdgeCuts => EDGE,
        _ => Color32::GRAY,
    }
}

/// A change to the board, made once the frame's input is read.
type BoardEdit = Box<dyn FnOnce(&mut Board) -> Result<(), String>>;

/// What the editor is doing with the pointer.
#[derive(Debug, Clone, PartialEq)]
pub enum PcbTool {
    Select,
    /// Routing: nothing yet (`net` empty), or a track of `net` running from `points`'
    /// last point.
    Route {
        net: String,
        width: f64,
        points: Vec<P>,
    },
    /// A copper pour being outlined.
    Zone {
        points: Vec<P>,
    },
    /// The board's edge being drawn.
    Outline {
        points: Vec<P>,
    },
    /// Measuring: from a point, to another (Ctrl+M).
    Measure {
        from: Option<P>,
        to: Option<P>,
    },
    /// A keepout being outlined.
    Keepout {
        points: Vec<P>,
    },
    /// A cutout through the board being outlined.
    Cutout {
        points: Vec<P>,
    },
    /// Placing round holes through the board: mounting holes.
    Hole,
}

/// What to do with the tool in hand, for the status bar.
pub fn hint(tool: &PcbTool) -> &'static str {
    match tool {
        PcbTool::Select => {
            "Click to select · drag the board's corners, or the dot mid-edge to bend it into an arc · double-click its edge to add a corner · drag a part (its tracks follow), a track (it slides square to itself), a corner or a via · R turn · F flip · L lock · Del delete · X route · Autoroute in the toolbar · PgUp/PgDn layer"
        }
        PcbTool::Route { points, .. } if points.is_empty() => {
            "Click a pad, track or via to start a track of its net · Esc to stop"
        }
        PcbTool::Route { .. } => {
            "Click to lay a corner · click a pad of the net to finish · V via and change layer · / corner style · Backspace undo a leg · Esc stop"
        }
        PcbTool::Zone { .. } => {
            "Click the pour's corners · double-click or Enter to finish · Backspace removes a corner · Esc cancels"
        }
        PcbTool::Outline { .. } => {
            "Click the board's corners · double-click or Enter to finish · Esc cancels"
        }
        PcbTool::Keepout { .. } => {
            "Click the keepout's corners · double-click or Enter to finish · what it bars is set in Properties"
        }
        PcbTool::Measure { .. } => {
            "Click where to measure from, then where to · the distance and its X and Y show · Esc done"
        }
        PcbTool::Cutout { .. } => {
            "Click the cutout's corners · double-click or Enter to finish · Esc cancels"
        }
        PcbTool::Hole => "Click to drill a hole of the diameter chosen in the toolbar · Esc done",
    }
}

impl PcbTool {
    pub fn name(&self) -> &'static str {
        match self {
            PcbTool::Select => "Select",
            PcbTool::Route { .. } => "Route",
            PcbTool::Zone { .. } => "Zone",
            PcbTool::Outline { .. } => "Outline",
            PcbTool::Measure { .. } => "Measure",
            PcbTool::Keepout { .. } => "Keepout",
            PcbTool::Cutout { .. } => "Cutout",
            PcbTool::Hole => "Hole",
        }
    }
}

/// Something on the board.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Footprint(String),
    Track(usize),
    Via(usize),
    Zone(usize),
    Keepout(usize),
    /// A corner of the board's outline.
    Corner(usize),
    /// A cutout: a board graphic on Edge.Cuts.
    Cutout(usize),
}

/// A board worked out from the design: resolved, poured, checked.
pub struct Analysis {
    pub cell: CellId,
    pub revision: u64,
    pub layout: Result<Layout, String>,
    pub checked: Option<Checked>,
    /// Each pour piece as triangles in board millimetres, with its layer.
    fills: Vec<(Layer, Vec<P>, Vec<u32>)>,
    /// The board's face, as triangles.
    face: (Vec<P>, Vec<u32>),
    /// Teardrops, worked out from the routing.
    teardrops: Vec<circuiter_pcb::teardrops::Teardrop>,
}

impl Analysis {
    pub fn new(design: &Design, cell: &CellId, revision: u64) -> Analysis {
        let board = design.cell(cell).and_then(|d| d.board.clone());
        let layout = match &board {
            Some(b) => Layout::new(design, cell, b),
            None => Err("no board yet".into()),
        };
        let checked = layout.as_ref().ok().map(|l| {
            let mut c = drc::check(l);
            c.violations.extend(drc::bodies(design, l));
            c
        });
        let tris = |outer: &Contour, holes: &[Contour]| {
            let f = |c: &Contour| c.iter().map(|q| (q[0] as f32, q[1] as f32)).collect::<Vec<_>>();
            let holes: Vec<_> = holes.iter().map(f).collect();
            let (pts, tris) = circuiter_3d::triangulate_holes(&f(outer), &holes);
            let pts: Vec<P> = pts.iter().map(|q| [f64::from(q.0), f64::from(q.1)]).collect();
            (pts, tris.into_iter().flatten().collect::<Vec<u32>>())
        };
        let mut fills = Vec::new();
        if let Some(c) = &checked {
            for f in &c.fills {
                for piece in &f.shapes {
                    if let Some((outer, holes)) = piece.split_first() {
                        let (pts, idx) = tris(outer, holes);
                        fills.push((f.layer, pts, idx));
                    }
                }
            }
        }
        let face = match &board {
            Some(b) if b.outline.len() >= 3 => {
                tris(&b.edge().iter().map(|q| p(*q)).collect(), &circuiter_pcb::zones::cutouts(b))
            }
            _ => (Vec::new(), Vec::new()),
        };
        let teardrops = layout.as_ref().map(circuiter_pcb::teardrops::shapes).unwrap_or_default();
        Analysis { cell: cell.clone(), revision, layout, checked, fills, face, teardrops }
    }
}

/// Which layers and markings show.
#[derive(Debug, Clone)]
pub struct Visible {
    /// Layers turned off.
    pub hidden: std::collections::BTreeSet<Layer>,
    pub zones: bool,
    pub ratsnest: bool,
    pub drc: bool,
    /// Other copper layers faded right down, so the one being worked on stands out.
    pub high_contrast: bool,
    /// The board as it comes from the board house: solder mask over the copper, gold
    /// pads, white legend; one side, the one being worked on.
    pub realistic: bool,
}

impl Visible {
    pub fn shows(&self, l: Layer) -> bool {
        !self.hidden.contains(&l)
    }

    pub fn toggle(&mut self, l: Layer) {
        if !self.hidden.remove(&l) {
            self.hidden.insert(l);
        }
    }
}

impl Default for Visible {
    fn default() -> Self {
        Visible {
            hidden: [Layer::FCrtYd, Layer::BCrtYd, Layer::FFab, Layer::BFab].into_iter().collect(),
            zones: true,
            ratsnest: true,
            drc: true,
            high_contrast: false,
            realistic: false,
        }
    }
}

/// What the interactive router does about obstacles, as Altium's conflict modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteMode {
    /// Pushes other nets' tracks aside to make room; around what can't be pushed.
    Shove,
    /// Finds a way round what's in the way.
    Walkaround,
    /// Stops short of the first thing in the way.
    Stop,
    /// Goes straight over it (the rules check will say).
    Ignore,
}

impl RouteMode {
    pub fn name(self) -> &'static str {
        match self {
            RouteMode::Shove => "Push and shove",
            RouteMode::Walkaround => "Walkaround",
            RouteMode::Stop => "Stop at obstacle",
            RouteMode::Ignore => "Ignore obstacles",
        }
    }

    fn next(self) -> RouteMode {
        match self {
            RouteMode::Shove => RouteMode::Walkaround,
            RouteMode::Walkaround => RouteMode::Stop,
            RouteMode::Stop => RouteMode::Ignore,
            RouteMode::Ignore => RouteMode::Shove,
        }
    }
}

/// The corners routes take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// 45° corners.
    Diagonal,
    /// 90° corners.
    Square,
    /// Any angle: straight from corner to corner.
    Any,
}

impl Corner {
    pub fn name(self) -> &'static str {
        match self {
            Corner::Diagonal => "45°",
            Corner::Square => "90°",
            Corner::Any => "any angle",
        }
    }

    fn next(self) -> Corner {
        match self {
            Corner::Diagonal => Corner::Square,
            Corner::Square => Corner::Any,
            Corner::Any => Corner::Diagonal,
        }
    }
}

/// Two legs from `a` to `b` in `corner`'s style, `flip` choosing which comes first.
pub fn legs(a: P, b: P, corner: Corner, flip: bool) -> Vec<P> {
    let mut out = match corner {
        Corner::Diagonal => return posture(a, b, flip),
        Corner::Square => {
            let c = if flip { [a[0], b[1]] } else { [b[0], a[1]] };
            vec![a, c, b]
        }
        Corner::Any => vec![a, b],
    };
    out.dedup_by(|x, y| (x[0] - y[0]).hypot(x[1] - y[1]) < 1e-9);
    out
}

/// What's being dragged with the select tool.
#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    Footprint(String),
    /// Several parts at once.
    Footprints(Vec<String>),
    /// A track's segment, moved square to itself.
    Segment(usize),
    /// A corner where tracks meet, on a layer.
    Corner(Layer, P),
    Via(usize),
    /// A corner of the board's outline.
    EdgeCorner(usize),
    /// The middle of the outline's edge from a corner: bending it into an arc.
    Bow(usize),
    /// A cutout, moved whole.
    Cutout(usize),
}

/// The layers a board has, for the Layers panel: copper, then the others.
pub const OTHER_LAYERS: &[(Layer, &str)] = &[
    (Layer::FSilk, "F.Silkscreen"),
    (Layer::BSilk, "B.Silkscreen"),
    (Layer::FCrtYd, "F.Courtyard"),
    (Layer::BCrtYd, "B.Courtyard"),
    (Layer::FFab, "F.Fab"),
    (Layer::BFab, "B.Fab"),
    (Layer::EdgeCuts, "Edge.Cuts"),
];

/// `board` with `drag` (from `from` to `to`) done to it: what a drag shows as it goes
/// and leaves when let go. Moves snap to `grid`.
pub fn apply_drag(board: &Board, layout: &Layout, drag: &Drag, from: P, to: P, grid: f64) -> Board {
    let mut b = board.clone();
    let snap = |v: f64| (v / grid).round() * grid;
    let delta = [snap(to[0] - from[0]), snap(to[1] - from[1])];
    match drag {
        Drag::Footprint(path) => {
            if let Some(f) = board.placed(path) {
                let at = p(f.at);
                let mut moved = f.clone();
                moved.at = point([snap(at[0] + delta[0]), snap(at[1] + delta[1])]);
                circuiter_pcb::edit::move_footprint(&mut b, layout, path, moved);
            }
        }
        Drag::Footprints(paths) => {
            for path in paths {
                if let Some(f) = board.placed(path) {
                    let at = p(f.at);
                    let mut moved = f.clone();
                    moved.at = point([snap(at[0] + delta[0]), snap(at[1] + delta[1])]);
                    circuiter_pcb::edit::move_footprint(&mut b, layout, path, moved);
                }
            }
        }
        Drag::Segment(k) => circuiter_pcb::edit::drag_segment(&mut b, *k, delta),
        Drag::Corner(layer, at) => {
            circuiter_pcb::edit::move_corner(&mut b, *layer, *at, [snap(to[0]), snap(to[1])])
        }
        Drag::Via(k) => circuiter_pcb::edit::move_via(&mut b, *k, [snap(to[0]), snap(to[1])]),
        Drag::EdgeCorner(k) => {
            if let Some(c) = b.outline.get_mut(*k) {
                *c = point([snap(to[0]), snap(to[1])]);
            }
        }
        Drag::Bow(k) => {
            let n = b.outline.len();
            if *k < n {
                let (a, c) = (p(b.outline[*k]), p(b.outline[(*k + 1) % n]));
                let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
                let chord = dx.hypot(dy);
                if chord > 0.0 {
                    // How far the pointer is to the edge's right, over half the chord;
                    // snapped straight near the chord.
                    let m = [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0];
                    let s = ((to[0] - m[0]) * dy - (to[1] - m[1]) * dx) / chord;
                    let s = if s.abs() < grid { 0.0 } else { snap(s) };
                    b.bulges.resize(n, 0.0);
                    b.bulges[*k] = (2.0 * s / chord).clamp(-8.0, 8.0);
                    b.tidy_bulges();
                }
            }
        }
        Drag::Cutout(k) => {
            let d = point(delta);
            match b.graphics.get_mut(*k) {
                Some(Graphic::Poly { points, .. }) => points.iter_mut().for_each(|q| *q = *q + d),
                Some(Graphic::Circle { center, .. }) => *center = *center + d,
                _ => {}
            }
        }
    }
    b
}

/// Whether a board graphic is a cutout, as [`Board::cutouts`] takes them.
fn cutout_graphic(g: &Graphic) -> bool {
    matches!(g, Graphic::Poly { layer: Layer::EdgeCuts, points, .. } if points.len() >= 3)
        || matches!(g, Graphic::Circle { layer: Layer::EdgeCuts, .. })
}

/// The handle of the board's outline at `at`: a corner, or an edge's middle (which
/// bends it).
pub fn edge_handle(board: &Board, at: P, tolerance: f64) -> Option<Drag> {
    let near = |q: P| (q[0] - at[0]).hypot(q[1] - at[1]) <= tolerance * 2.0;
    if let Some(k) = board.outline.iter().position(|q| near(p(*q))) {
        return Some(Drag::EdgeCorner(k));
    }
    (0..board.outline.len()).find(|&k| near(edge_middle(board, k))).map(Drag::Bow)
}

/// The middle of the outline's edge from corner `k`, on its arc if it is one.
pub fn edge_middle(board: &Board, k: usize) -> P {
    let n = board.outline.len();
    let (a, b) = (board.outline[k], board.outline[(k + 1) % n]);
    let bulge = board.bulges.get(k).copied().unwrap_or(0.0);
    circuiter_model::board::arc_middle(a, b, bulge).map_or_else(
        || {
            let (a, b) = (p(a), p(b));
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        },
        p,
    )
}

/// The outline's edge passing within `tolerance` of `at`, as the corner it starts from.
fn edge_near(board: &Board, at: P, tolerance: f64) -> Option<usize> {
    let n = board.outline.len();
    (0..n).find(|&k| {
        let ends = [board.outline[k], board.outline[(k + 1) % n]];
        let bulge = board.bulges.get(k).copied().unwrap_or(0.0);
        let e: Vec<P> = circuiter_model::board::arc_outline(&ends, &[bulge, 0.0])
            .iter()
            .map(|q| p(*q))
            .collect();
        e.windows(2).any(|w| geometry::segment_distance(w[0], w[1], at, at) <= tolerance * 1.5)
    })
}

/// A route worked out: what it was asked (as a key), the path and whether it's blocked,
/// and the board with tracks shoved aside for it.
type RouteCache = (String, (Vec<P>, bool), Option<Board>);

/// A panel made up: its size and how many copies, or why it can't be.
type PanelMade = Result<((f64, f64), usize), String>;

/// Power analysis: what the simulated operating point's currents do to the copper.
#[derive(Debug, Clone)]
pub struct PowerView {
    /// Each net carrying current, worst drop first.
    pub nets: Vec<circuiter_pcb::power::NetPower>,
    /// The net whose tracks are coloured by how hard they're worked.
    pub shown: Option<String>,
    /// Why there are no results.
    pub why: Option<String>,
    /// The board's revision analysed: its tracks are numbered as then.
    pub revision: u64,
    /// The calculator: a current, and the temperature rise allowed.
    pub amps: f64,
    pub rise: f64,
}

impl Default for PowerView {
    fn default() -> Self {
        PowerView { nets: Vec::new(), shown: None, why: None, revision: 0, amps: 1.0, rise: 10.0 }
    }
}

impl PowerView {
    /// How hard each of the shown net's tracks is worked: its current over what
    /// IPC-2221 allows for the rise.
    fn load(&self, layout: &Layout) -> std::collections::BTreeMap<usize, f64> {
        let t = circuiter_pcb::power::copper_thickness(layout);
        let Some(net) = self.nets.iter().find(|n| Some(&n.net) == self.shown.as_ref()) else {
            return Default::default();
        };
        net.tracks
            .iter()
            .filter_map(|&(k, amps)| {
                let tr = layout.board.tracks.get(k)?;
                let outer = matches!(tr.layer, Layer::FCu | Layer::BCu);
                let cap =
                    circuiter_pcb::power::current_for_width(tr.width.mm(), t, self.rise, outer);
                Some((k, amps / cap))
            })
            .collect()
    }
}

/// From cool to hot: a track's share of what it can carry.
fn heat(load: f64) -> Color32 {
    let t = load.clamp(0.0, 1.2) / 1.2;
    let stops =
        [(0.0, [40, 90, 220]), (0.4, [40, 200, 120]), (0.7, [240, 210, 40]), (1.0, [235, 40, 40])];
    let k = stops.windows(2).position(|w| t <= w[1].0).unwrap_or(stops.len() - 2);
    let ((t0, a), (t1, b)) = (stops[k], stops[k + 1]);
    let f = (t - t0) / (t1 - t0);
    let mix = |i: usize| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * f) as u8;
    Color32::from_rgb(mix(0), mix(1), mix(2))
}

/// The board editor's state.
pub struct PcbView {
    pub camera: Option<Camera>,
    pub analysis: Option<Analysis>,
    pub tool: PcbTool,
    /// The copper layer being worked on.
    pub layer: Layer,
    pub selected: Option<Pick>,
    /// What's being dragged, where from and to, and the board as it'll be.
    drag: Option<(Drag, P, P)>,
    preview: Option<Board>,
    pub visible: Visible,
    /// The Layers panel beside the board.
    pub show_layers: bool,
    /// A layer picked (in the panel) while routing: a via takes the track there.
    pending_layer: Option<Layer>,
    /// How the interactive router deals with what's in the way, and its corners.
    pub mode: RouteMode,
    pub corner: Corner,
    /// The routing grid for the board as it is (rebuilt when it changes).
    router: Option<(CellId, u64, Router)>,
    /// The route to the pointer as it would be laid, and whether it's blocked.
    pub route_preview: Option<(Vec<P>, bool)>,
    /// Tracks laid in the route under way (kept by loop removal).
    route_laid: Vec<Track>,
    /// The last route worked out, and what it was worked out for.
    route_cache: Option<RouteCache>,
    /// Pushing and shoving: the board with other nets' tracks moved aside for the route
    /// shown.
    pub shoved: Option<Board>,
    /// Only the layer being worked on shows (Shift+S).
    pub single_layer: bool,
    /// Net colours shown on the routing (F5).
    pub net_colours: bool,
    /// The net picked in the Nets panel, shown bright.
    pub highlight: Option<String>,
    /// Which tab of the panel under the board shows.
    pub bottom_tab: usize,
    /// The schematic part last seen selected, to follow it (cross-select).
    cross: Option<String>,
    /// Ctrl+Shift+wheel travel, a layer per notch.
    wheel: f32,
    /// Parts selected together (box or Shift+click), for moving and aligning.
    pub group: std::collections::BTreeSet<String>,
    /// The selection box being dragged out.
    band: Option<(P, P)>,
    /// The Engineering Change Order being reviewed: each change and whether to make it.
    pub eco: Option<Vec<(bool, circuiter_pcb::sync::Change)>>,
    /// Via stitching being set up: the net and the spacing.
    pub stitching: Option<(String, f64)>,
    /// Length tuning being set up: target length, tallest bump, bump spacing.
    pub tuning: Option<(f64, f64, f64)>,
    /// The Power Analysis window, while open.
    pub power: Option<PowerView>,
    /// The panel made up, for the board's revision: its size and copies, or why not.
    panel_made: Option<(u64, PanelMade)>,
    pub grid: f64,
    /// The diameter of holes the Hole tool drills, in millimetres.
    pub hole: f64,
    /// The radius Round Corners gives the board's corners, in millimetres.
    pub corner_radius: f64,
    /// The net new pours are of.
    pub zone_net: String,
    /// Route corners: diagonal first, then straight (`/` swaps).
    pub diagonal_first: bool,
    /// A note for the status line (what was exported, why a route didn't start).
    pub note: Option<String>,
    cursor: Option<P>,
    /// Where the board was drawn last, on screen.
    pub rect: Option<egui::Rect>,
}

impl Default for PcbView {
    fn default() -> Self {
        PcbView {
            camera: None,
            analysis: None,
            tool: PcbTool::Select,
            layer: Layer::FCu,
            selected: None,
            drag: None,
            preview: None,
            visible: Visible::default(),
            show_layers: true,
            pending_layer: None,
            mode: RouteMode::Shove,
            corner: Corner::Diagonal,
            router: None,
            route_preview: None,
            route_laid: Vec::new(),
            route_cache: None,
            shoved: None,
            single_layer: false,
            net_colours: false,
            highlight: None,
            bottom_tab: 0,
            cross: None,
            wheel: 0.0,
            group: Default::default(),
            band: None,
            eco: None,
            stitching: None,
            tuning: None,
            power: None,
            panel_made: None,
            grid: 0.635,
            hole: 3.2,
            corner_radius: 3.0,
            zone_net: String::new(),
            diagonal_first: false,
            note: None,
            cursor: None,
            rect: None,
        }
    }
}

impl PcbView {
    /// The analysis of `cell`'s board, recomputed when the design changed.
    pub fn update(&mut self, design: &Design, cell: &CellId, revision: u64) {
        let fresh =
            self.analysis.as_ref().is_some_and(|a| a.cell == *cell && a.revision == revision);
        if !fresh {
            if self.analysis.as_ref().is_some_and(|a| a.cell != *cell) {
                self.camera = None;
                self.selected = None;
                self.tool = PcbTool::Select;
            }
            self.analysis = Some(Analysis::new(design, cell, revision));
        }
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.analysis.as_ref()?.layout.as_ref().ok()
    }

    /// The routing grid for the board as it is now.
    fn router(&mut self) -> Option<&mut Router> {
        let a = self.analysis.as_ref()?;
        let layout = a.layout.as_ref().ok()?;
        let fresh = self.router.as_ref().is_some_and(|(c, r, _)| *c == a.cell && *r == a.revision);
        if !fresh {
            let router = Router::new(layout, 0.15)?;
            self.router = Some((a.cell.clone(), a.revision, router));
        }
        self.router.as_mut().map(|(_, _, r)| r)
    }

    /// The track from `from` to `to` for `net` on `layer` as the router would lay it,
    /// by the mode and corner style; and whether something's in the way.
    fn route_to(&mut self, net: &str, width: f64, layer: Layer, from: P, to: P) -> (Vec<P>, bool) {
        // Worked out again only when something it depends on changed.
        let key = format!(
            "{net} {width} {layer:?} {from:?} {to:?} {:?} {:?} {} {:?}",
            self.mode,
            self.corner,
            self.diagonal_first,
            self.analysis.as_ref().map(|a| a.revision)
        );
        if let Some((k, found, shoved)) = &self.route_cache {
            if *k == key {
                self.shoved = shoved.clone();
                return found.clone();
            }
        }
        self.shoved = None;
        let found = self.route_uncached(net, width, layer, from, to);
        self.route_cache = Some((key, found.clone(), self.shoved.clone()));
        found
    }

    fn route_uncached(
        &mut self,
        net: &str,
        width: f64,
        layer: Layer,
        from: P,
        to: P,
    ) -> (Vec<P>, bool) {
        let (mode, corner, flip) = (self.mode, self.corner, self.diagonal_first);
        let straight = legs(from, to, corner, flip);
        let Some(layout) = self.layout().cloned() else { return (straight, false) };
        match mode {
            RouteMode::Ignore => {
                let blocked = route_margin(&layout, net, &straight, width, layer) < -1e-6;
                (straight, blocked)
            }
            RouteMode::Stop => {
                let Some(router) = self.router() else { return (straight, false) };
                let cut = router.until_blocked(net, layer, &straight);
                let end = *cut.last().expect("a start");
                let short = (end[0] - to[0]).hypot(end[1] - to[1]) > 1e-6;
                (cut, short)
            }
            RouteMode::Shove | RouteMode::Walkaround => {
                // Straight if nothing's in the way; else pushing it aside; else around it.
                if route_margin(&layout, net, &straight, width, layer) >= -1e-6 {
                    return (straight, false);
                }
                if mode == RouteMode::Shove {
                    if let Some(b) =
                        circuiter_pcb::edit::shove(&layout, net, &straight, width, layer)
                    {
                        self.shoved = Some(b);
                        return (straight, false);
                    }
                }
                let opts = RouteOptions {
                    vias: false,
                    orthogonal: corner == Corner::Square,
                    any_angle: corner == Corner::Any,
                    limit: 400_000,
                    against: 0.0,
                    ..RouteOptions::default()
                };
                let Some(router) = self.router() else { return (straight, true) };
                match router.connect(net, from, &[layer], to, &[layer], width, &opts) {
                    Some((tracks, _)) if !tracks.is_empty() => {
                        let mut pts = vec![p(tracks[0].a)];
                        pts.extend(tracks.iter().map(|t| p(t.b)));
                        (pts, false)
                    }
                    _ => (straight, true),
                }
            }
        }
    }

    pub fn snap(&self, v: P) -> P {
        let g = self.grid;
        [(v[0] / g).round() * g, (v[1] / g).round() * g]
    }
}

/// Two legs at 45°: straight then diagonal, or diagonal then straight.
pub fn posture(a: P, b: P, diagonal_first: bool) -> Vec<P> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d = dx.abs().min(dy.abs());
    let diag = [d * dx.signum(), d * dy.signum()];
    let corner = if diagonal_first {
        [a[0] + diag[0], a[1] + diag[1]]
    } else {
        [b[0] - diag[0], b[1] - diag[1]]
    };
    let mut out = vec![a];
    let near = |u: P, v: P| (u[0] - v[0]).abs() < 1e-9 && (u[1] - v[1]).abs() < 1e-9;
    if !near(corner, a) && !near(corner, b) {
        out.push(corner);
    }
    if !near(b, a) {
        out.push(b);
    }
    out
}

fn inside(poly: &[P], q: P) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            odd = !odd;
        }
    }
    odd
}

/// The pad under `at`, if any: on `layer` when one is given.
pub fn pad_at(layout: &Layout, at: P, layer: Option<Layer>) -> Option<&PadAt> {
    layout.pads.iter().find(|pad| {
        layer.is_none_or(|l| pad.layers.contains(&l)) && inside(&geometry::pad(pad, 0.0), at)
    })
}

/// What's under `at`: tracks and vias first, then parts, then pours.
pub fn pick(layout: &Layout, at: P, tolerance: f64, layer: Layer) -> Option<Pick> {
    let board = &layout.board;
    let near_track = |t: &Track| {
        geometry::segment_distance(p(t.a), p(t.b), at, at) <= t.width.mm() / 2.0 + tolerance
    };
    for want in [true, false] {
        if let Some(k) =
            board.tracks.iter().position(|t| (t.layer == layer) == want && near_track(t))
        {
            return Some(Pick::Track(k));
        }
    }
    let via = board.vias.iter().position(|v| {
        let c = p(v.at);
        (c[0] - at[0]).hypot(c[1] - at[1]) <= v.diameter.mm() / 2.0 + tolerance
    });
    if let Some(k) = via {
        return Some(Pick::Via(k));
    }
    // The smallest part whose box holds the point.
    let mut best: Option<(f64, &str)> = None;
    for part in &layout.parts {
        let Some(place) = part.place else { continue };
        let (lo, hi) = sync::extent(&part.footprint);
        let q = place.unapply(at);
        if q[0] >= lo[0] && q[0] <= hi[0] && q[1] >= lo[1] && q[1] <= hi[1] {
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            if best.is_none_or(|(a, _)| area < a) {
                best = Some((area, &part.path));
            }
        }
    }
    if let Some((_, path)) = best {
        return Some(Pick::Footprint(path.to_owned()));
    }
    for (k, g) in board.graphics.iter().enumerate() {
        let hit = match g {
            Graphic::Poly { layer: Layer::EdgeCuts, points, .. } => {
                inside(&points.iter().map(|q| p(*q)).collect::<Vec<_>>(), at)
            }
            Graphic::Circle { layer: Layer::EdgeCuts, center, radius, .. } => {
                let c = p(*center);
                (c[0] - at[0]).hypot(c[1] - at[1]) <= radius.mm() + tolerance
            }
            _ => false,
        };
        if hit {
            return Some(Pick::Cutout(k));
        }
    }
    if let Some(k) = board
        .keepouts
        .iter()
        .position(|z| inside(&z.outline.iter().map(|q| p(*q)).collect::<Vec<_>>(), at))
    {
        return Some(Pick::Keepout(k));
    }
    board
        .zones
        .iter()
        .position(|z| inside(&z.outline.iter().map(|q| p(*q)).collect::<Vec<_>>(), at))
        .map(Pick::Zone)
}

/// The net of the copper at `at` on `layer` (a pad, track or via) and the point a route
/// should start from there.
fn copper_at(layout: &Layout, at: P, tolerance: f64, layer: Layer) -> Option<(Option<String>, P)> {
    if let Some(pad) = pad_at(layout, at, Some(layer)) {
        return Some((pad.net.clone(), pad.center));
    }
    let board = &layout.board;
    for v in &board.vias {
        let c = p(v.at);
        if (c[0] - at[0]).hypot(c[1] - at[1]) <= v.diameter.mm() / 2.0 + tolerance {
            return Some((Some(v.net.clone()), c));
        }
    }
    for t in board.tracks.iter().filter(|t| t.layer == layer) {
        let (a, b) = (p(t.a), p(t.b));
        if geometry::segment_distance(a, b, at, at) <= t.width.mm() / 2.0 + tolerance {
            // An end if near one, else the nearest point along it.
            let end =
                [a, b].into_iter().find(|e| (e[0] - at[0]).hypot(e[1] - at[1]) <= t.width.mm());
            let on = end.unwrap_or_else(|| {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = (dx * dx + dy * dy).max(1e-12);
                let s = (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                [a[0] + s * dx, a[1] + s * dy]
            });
            return Some((Some(t.net.clone()), on));
        }
    }
    None
}

/// The closest a route of `net`, `width` wide along `path` on `layer`, comes to copper
/// of other nets, less the clearance it needs: negative where it breaks the rule.
fn route_margin(layout: &Layout, net: &str, path: &[P], width: f64, layer: Layer) -> f64 {
    circuiter_pcb::edit::margin(layout, net, path, width, layer)
}

/// Draws board millimetres on screen.
struct Draw<'a> {
    painter: &'a egui::Painter,
    cam: Camera,
    view: egui::Rect,
}

impl Draw<'_> {
    fn at(&self, v: P) -> Pos2 {
        self.cam.to_screen(self.view, Pt::new(v[0], v[1]))
    }

    fn px(&self, mm: f64) -> f32 {
        (mm as f32 * self.cam.scale).max(1.0)
    }

    fn convex(&self, c: &[P], fill: Color32) {
        let pts: Vec<Pos2> = c.iter().map(|&q| self.at(q)).collect();
        self.painter.add(egui::Shape::convex_polygon(pts, fill, Stroke::NONE));
    }

    fn outline(&self, c: &[P], stroke: Stroke) {
        let pts: Vec<Pos2> = c.iter().map(|&q| self.at(q)).collect();
        self.painter.add(egui::Shape::closed_line(pts, stroke));
    }

    fn mesh(&self, pts: &[P], idx: &[u32], fill: Color32) {
        let mut m = egui::Mesh::default();
        for &q in pts {
            m.colored_vertex(self.at(q), fill);
        }
        m.indices = idx.to_vec();
        self.painter.add(egui::Shape::mesh(m));
    }

    /// A round-ended stroke `width` millimetres wide.
    fn stroke(&self, pts: &[P], width: f64, colour: Color32) {
        let w = self.px(width);
        for seg in pts.windows(2) {
            self.painter.line_segment([self.at(seg[0]), self.at(seg[1])], Stroke::new(w, colour));
        }
        if w > 2.0 {
            for &q in pts {
                self.painter.circle_filled(self.at(q), w / 2.0, colour);
            }
        }
    }

    fn disc(&self, c: P, r: f64, fill: Color32) {
        self.painter.circle_filled(self.at(c), (r as f32 * self.cam.scale).max(0.5), fill);
    }
}

impl CircuiterApp {
    /// The board view: toolbar, then the board.
    pub(super) fn view_pcb(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &self.workspace else { return };
        let cell = ws.current().cell.clone();
        self.pcb.update(&ws.design, &cell, ws.revision);
        let has_schematic = ws.has_schematic();
        let editable = ws.editable();
        self.pcb_toolbar(ui);
        let p = style::palette(ui);
        let has_board = self.pcb.analysis.as_ref().is_some_and(|a| a.layout.is_ok());
        if has_board && self.pcb.show_layers {
            egui::Panel::left("pcb-layers")
                .resizable(true)
                .default_size(220.0)
                .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::same(8)))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.pcb_layers(ui))
                });
        }
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        if !has_board {
            let p = &style::DARK;
            ui.painter().rect_filled(rect, 0.0, BACKGROUND);
            let reason = self
                .pcb
                .analysis
                .as_ref()
                .and_then(|a| a.layout.as_ref().err().cloned())
                .unwrap_or_default();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_center_size(
                        rect.center(),
                        egui::vec2((rect.width() - 48.0).clamp(0.0, 480.0), rect.height()),
                    ))
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            child.add_space((rect.height() * 0.25).max(12.0));
            child.label(RichText::new(icon::CPU).size(42.0).color(p.accent));
            child.add_space(12.0);
            if reason == "no board yet" {
                child.label(
                    RichText::new(format!("{} has no board yet", cell.cell))
                        .size(22.0)
                        .strong()
                        .color(p.text),
                );
                child.add_space(6.0);
                if has_schematic {
                    child.label(RichText::new("A board is made from the schematic: every part with a footprint is placed on it, joined by the ratsnest, ready to route.").color(p.weak));
                    child.add_space(10.0);
                    let make = egui::Button::new(
                        RichText::new(format!("{}  Create Board from Schematic", icon::SPARKLE))
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(p.accent)
                    .corner_radius(7)
                    .min_size(egui::vec2(0.0, 36.0));
                    if child.add_enabled(editable, make).clicked() {
                        self.pcb_create_board();
                    }
                } else {
                    child.label(
                        RichText::new("Boards are made from schematics: open a cell with one.")
                            .color(p.weak),
                    );
                }
            } else {
                child.label(
                    RichText::new(format!("Can't lay out the board: {reason}")).color(p.error),
                );
            }
            return;
        }
        self.pcb_cross_select(true);
        self.pcb_interact(ui, rect, &response);
        self.pcb_cross_select(false);
        self.pcb_paint(ui, rect);
        self.pcb_windows(&ui.ctx().clone());
    }

    /// Cross-select, as Altium's: a part selected on the schematic is selected on the
    /// board (`inbound`), and one selected on the board is selected on the schematic.
    fn pcb_cross_select(&mut self, inbound: bool) {
        let Some(ws) = &mut self.workspace else { return };
        let Some(schematic) = ws.schematic() else { return };
        let on_sheet = match ws.selection.iter().collect::<Vec<_>>()[..] {
            [circuiter_scene::Owner::Instance(i)] => {
                schematic.instances.get(*i).map(|x| x.name.clone())
            }
            _ => None,
        };
        if inbound {
            if on_sheet.is_some() && on_sheet != self.pcb.cross {
                self.pcb.selected = on_sheet.clone().map(Pick::Footprint);
                self.pcb.cross = on_sheet;
            }
            return;
        }
        if let Some(Pick::Footprint(path)) = &self.pcb.selected {
            if Some(path) != on_sheet.as_ref() {
                if let Some(i) = schematic.instances.iter().position(|x| x.name == *path) {
                    ws.select_only(Some(circuiter_scene::Owner::Instance(i)));
                    self.pcb.cross = Some(path.clone());
                }
            }
        }
    }

    /// The Layers panel: every layer with its colour, shown or hidden, and the copper
    /// layer being worked on.
    fn pcb_layers(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let Some(layout) = self.pcb.layout() else { return };
        let copper = layout.board.copper();
        let count = layout.board.copper_layers;
        let editable = self.workspace.as_ref().is_some_and(|ws| ws.editable());
        let mut set_count = None;
        let view = &mut self.pcb;
        ui.label(RichText::new(format!("{}  Layers", icon::STACK)).strong().size(15.0));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Copper").color(p.weak));
            ui.add_enabled_ui(editable, |ui| {
                egui::ComboBox::from_id_salt("copper-count")
                    .selected_text(format!("{count} layers"))
                    .show_ui(ui, |ui| {
                        for n in [2u8, 4, 6] {
                            if ui.selectable_label(n == count, format!("{n} layers")).clicked()
                                && n != count
                            {
                                set_count = Some(n);
                            }
                        }
                    });
            });
        });
        // A row: its colour, an eye, its name (click to work on it).
        let row =
            |ui: &mut egui::Ui, l: Layer, name: &str, view: &mut PcbView, copper: bool| -> bool {
                let mut pick = false;
                ui.horizontal(|ui| {
                    let shown = view.visible.shows(l);
                    let eye = if shown { icon::EYE } else { icon::EYE_SLASH };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(eye).color(if shown {
                                p.text
                            } else {
                                p.weak
                            }))
                            .frame(false),
                        )
                        .on_hover_text(if shown { "Hide" } else { "Show" })
                        .clicked()
                    {
                        view.visible.toggle(l);
                    }
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, layer_colour(l));
                    let active = copper && view.layer == l;
                    let text = RichText::new(name).color(if shown { p.text } else { p.weak });
                    let text = if active { text.strong() } else { text };
                    if copper {
                        if ui
                            .selectable_label(active, text)
                            .on_hover_text("Work on this layer: routing, pours")
                            .clicked()
                        {
                            pick = true;
                        }
                    } else {
                        ui.label(text);
                    }
                });
                pick
            };
        for &l in &copper {
            let name = match l {
                Layer::FCu => "F.Cu  (front)".to_owned(),
                Layer::BCu => "B.Cu  (back)".to_owned(),
                other => other.name(),
            };
            if row(ui, l, &name, view, true) && view.layer != l {
                let routing =
                    matches!(&view.tool, PcbTool::Route { points, .. } if !points.is_empty());
                if routing {
                    view.pending_layer = Some(l);
                } else {
                    view.layer = l;
                }
                view.visible.hidden.remove(&l);
            }
        }
        ui.separator();
        for &(l, name) in OTHER_LAYERS {
            row(ui, l, name, view, false);
        }
        ui.separator();
        ui.checkbox(&mut view.visible.zones, "Copper pours");
        ui.checkbox(&mut view.visible.ratsnest, "Ratsnest");
        ui.checkbox(&mut view.visible.drc, "Rule markers");
        ui.checkbox(&mut view.visible.high_contrast, "Dim other layers");
        ui.add_space(6.0);
        ui.label(
            RichText::new("Click a copper layer to route on it; while routing, clicking another drops a via to it. PgUp/PgDn step through the layers; V drops a via and goes to the other side.")
                .small()
                .color(p.weak),
        );
        if let Some(n) = set_count {
            let Some(ws) = &mut self.workspace else { return };
            let done = ws.edit_board(|_, _, b| {
                let b = b.as_mut().ok_or("no board")?;
                let gone: Vec<Layer> = b
                    .copper()
                    .into_iter()
                    .filter(|l| matches!(l, Layer::In(k) if *k + 2 > n))
                    .collect();
                if b.tracks.iter().any(|t| gone.contains(&t.layer))
                    || b.zones.iter().any(|z| gone.contains(&z.layer))
                {
                    return Err(
                        "tracks or pours are on the inner layers that would go: remove them first"
                            .into(),
                    );
                }
                b.copper_layers = n;
                Ok(())
            });
            match done {
                Ok(()) => {
                    if !self
                        .pcb
                        .layout()
                        .is_some_and(|l| l.board.copper().contains(&self.pcb.layer))
                    {
                        self.pcb.layer = Layer::FCu;
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Ctrl+click: routes from `from` to the nearest copper of `net` not yet joined to
    /// it, through vias if it must; the edit to make, if a way was found.
    fn pcb_auto_complete(
        &mut self,
        layout: &Layout,
        net: &str,
        from: P,
        width: f64,
    ) -> Option<BoardEdit> {
        let checked = self.pcb.analysis.as_ref()?.checked.as_ref()?;
        let probe = vec![vec![geometry::circle(from, 0.05)]];
        let here: Vec<usize> = checked
            .copper
            .iter()
            .enumerate()
            .filter(|(_, c)| c.net.as_deref() == Some(net) && geometry::overlaps(&c.shape, &probe))
            .map(|(k, _)| checked.connectivity.island[k])
            .collect();
        let target = checked
            .copper
            .iter()
            .enumerate()
            .filter(|(k, c)| {
                c.net.as_deref() == Some(net) && !here.contains(&checked.connectivity.island[*k])
            })
            .flat_map(|(_, c)| c.anchors.iter().copied())
            .min_by(|a, b| {
                let d = |q: &P| (q[0] - from[0]).hypot(q[1] - from[1]);
                d(a).total_cmp(&d(b))
            });
        let Some(to) = target else {
            self.pcb.note = Some(format!("{net} is already connected"));
            return None;
        };
        let mut la = circuiter_pcb::route::layers_of(layout, from, net);
        if !la.contains(&self.pcb.layer) {
            la.push(self.pcb.layer);
        }
        let lb = circuiter_pcb::route::layers_of(layout, to, net);
        let opts = RouteOptions {
            orthogonal: self.pcb.corner == Corner::Square,
            any_angle: self.pcb.corner == Corner::Any,
            ..RouteOptions::default()
        };
        let found = self.pcb.router()?.connect(net, from, &la, to, &lb, width, &opts);
        let Some((mut tracks, vias)) = found else {
            self.pcb.note =
                Some(format!("No way through for {net}: route it by hand, or move parts apart"));
            return None;
        };
        for t in &mut tracks {
            t.width = Length::from_mm(width);
        }
        let mut laid = std::mem::take(&mut self.pcb.route_laid);
        laid.extend(tracks.iter().cloned());
        let length: f64 =
            tracks.iter().map(|t| (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm())).sum();
        self.pcb.note =
            Some(format!("Auto-completed {net}: {length:.2} mm, {} via(s)", vias.len()));
        self.pcb.tool = PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
        self.pcb.route_preview = None;
        let (layout, net) = (layout.clone(), net.to_owned());
        Some(Box::new(move |b: &mut Board| {
            b.tracks.extend(tracks);
            b.vias.extend(vias);
            circuiter_pcb::edit::remove_loops(b, &layout, &net, &laid);
            Ok(())
        }))
    }

    /// Routes the ratsnest automatically: of `nets`, or everything.
    pub(super) fn pcb_autoroute(&mut self, nets: Option<std::collections::BTreeSet<String>>) {
        let Some(layout) = self.pcb.layout().cloned() else { return };
        let opts = circuiter_pcb::route::Options { nets, ..Default::default() };
        let routed = circuiter_pcb::route::autoroute(&layout, &opts);
        if routed.done == 0 && routed.failed.is_empty() {
            self.pcb.note = Some("Nothing to route: every connection is made".into());
            return;
        }
        let Some(ws) = &mut self.workspace else { return };
        let (tracks, vias) = (routed.tracks.clone(), routed.vias.clone());
        let done = ws.edit_board(|_, _, b| {
            let b = b.as_mut().ok_or("no board")?;
            b.tracks.extend(tracks);
            b.vias.extend(vias);
            Ok(())
        });
        if let Err(e) = done {
            self.error = Some(e);
            return;
        }
        self.pcb.note = Some(if routed.failed.is_empty() {
            format!("Routed {} connection(s)", routed.done)
        } else {
            let nets: std::collections::BTreeSet<&str> =
                routed.failed.iter().map(|f| f.0.as_str()).collect();
            format!(
                "Routed {}; {} left on the ratsnest (no way through for {}): move parts apart or route them by hand",
                routed.done,
                routed.failed.len(),
                nets.into_iter().collect::<Vec<_>>().join(", ")
            )
        });
    }

    /// The nets the selection is on: a track's, a via's, a part's pads'.
    fn pcb_selected_nets(&self) -> Option<std::collections::BTreeSet<String>> {
        let layout = self.pcb.layout()?;
        let nets: std::collections::BTreeSet<String> = match self.pcb.selected.as_ref()? {
            Pick::Track(k) => {
                layout.board.tracks.get(*k).map(|t| t.net.clone()).into_iter().collect()
            }
            Pick::Via(k) => layout.board.vias.get(*k).map(|v| v.net.clone()).into_iter().collect(),
            Pick::Zone(k) => {
                layout.board.zones.get(*k).map(|z| z.net.clone()).into_iter().collect()
            }
            Pick::Keepout(_) | Pick::Corner(_) | Pick::Cutout(_) => Default::default(),
            Pick::Footprint(path) => {
                let k = layout.part(path)?;
                layout.pads_of(k).filter_map(|pad| pad.net.clone()).collect()
            }
        };
        (!nets.is_empty()).then_some(nets)
    }

    /// Edits the board with tracks: tidy them, or take a net's (or all) off.
    fn pcb_tracks(&mut self, action: &str) {
        let Some(layout) = self.pcb.layout().cloned() else { return };
        let nets = self.pcb_selected_nets();
        let Some(ws) = &mut self.workspace else { return };
        let mut note = String::new();
        let done = ws.edit_board(|_, _, b| {
            let b = b.as_mut().ok_or("no board")?;
            match action {
                "clean" => {
                    let n = circuiter_pcb::edit::clean_up(b, &layout);
                    note = if n == 0 {
                        "The tracks are already tidy".into()
                    } else {
                        format!("Tidied {n} track segment(s)")
                    };
                }
                "unroute-net" => {
                    for n in nets.iter().flatten() {
                        circuiter_pcb::edit::unroute(b, Some(n));
                    }
                    note = "Tracks taken off".into();
                }
                _ => {
                    circuiter_pcb::edit::unroute(b, None);
                    note = "Every track and via taken off".into();
                }
            }
            Ok(())
        });
        match done {
            Ok(()) => self.pcb.note = Some(note),
            Err(e) => self.error = Some(e),
        }
        self.pcb.selected = None;
    }

    /// Makes the cell a board from its schematic, every part placed.
    pub(super) fn pcb_create_board(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let made = ws.edit_board(|design, id, board| {
            *board = Some(sync::new_board(design, id)?);
            Ok(())
        });
        match made {
            Ok(()) => {
                self.pcb.camera = None;
                self.pcb.note =
                    Some("Board made: parts placed, ratsnest showing. Route with X.".into());
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Turns the case made to fit into two 3D parts of the project, base and lid, and
    /// uses them: they can then be changed in the body editor.
    pub(super) fn pcb_case_parts(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let cell = ws.current().cell.clone();
        let made = (|| -> Result<_, String> {
            let board = ws.current_def().and_then(|d| d.board.clone()).ok_or("no board")?;
            let e = board.enclosure.clone().ok_or("no case")?;
            let layout = Layout::new(&ws.design, &cell, &board)?;
            let built = circuiter_pcb::board3d::build(&ws.design, &layout, &[]);
            circuiter_pcb::enclosure::make(&layout, &built, &e)
        })();
        let made = match made {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let result = (|| -> Result<(CellId, CellId), String> {
            let base =
                ws.add_body_part(&format!("{}_case", cell.cell), "enclosures", made.base.clone())?;
            let lid =
                ws.add_body_part(&format!("{}_lid", cell.cell), "enclosures", made.lid.clone())?;
            ws.edit_board(|_, _, b| {
                let e = b.as_mut().and_then(|b| b.enclosure.as_mut()).ok_or("no case")?;
                e.part = Some(circuiter_model::CellRef::new(None, &base.cell));
                e.lid = Some(circuiter_model::CellRef::new(None, &lid.cell));
                e.board_at = [0.0; 3];
                Ok(())
            })?;
            Ok((base, lid))
        })();
        match result {
            Ok((base, lid)) => {
                self.pcb.note = Some(format!(
                    "The case is now {} and {}: open them to model",
                    base.cell, lid.cell
                ));
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Opens the Engineering Change Order: what updating from the schematic would do,
    /// each change to tick or untick before it's made.
    pub(super) fn pcb_update_from_schematic(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let cell = ws.current().cell.clone();
        let Some(board) = ws.current_def().and_then(|d| d.board.clone()) else {
            if let Err(e) = ws.edit_board(|design, id, board| {
                *board = Some(sync::new_board(design, id)?);
                Ok(())
            }) {
                self.error = Some(e);
            }
            return;
        };
        match sync::changes(&ws.design, &cell, &board) {
            Ok(c) if c.is_empty() => self.pcb.note = Some("The board matches the schematic".into()),
            Ok(c) => self.pcb.eco = Some(c.into_iter().map(|c| (true, c)).collect()),
            Err(e) => self.error = Some(e),
        }
    }

    /// The board tools' windows: the Engineering Change Order, via stitching, length
    /// tuning.
    fn pcb_windows(&mut self, ctx: &egui::Context) {
        let mut run: Option<&str> = None;
        if let Some(eco) = &mut self.pcb.eco {
            let mut open = true;
            egui::Window::new("Engineering Change Order").open(&mut open).collapsible(false).resizable(true).show(ctx, |ui| {
                ui.label("Updating the board from the schematic will make these changes. Untick any to leave out.");
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (on, c) in eco.iter_mut() {
                        ui.checkbox(on, c.describe());
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Execute Changes").strong()).clicked() {
                        run = Some("eco");
                    }
                    if ui.button("All").clicked() {
                        eco.iter_mut().for_each(|(on, _)| *on = true);
                    }
                    if ui.button("None").clicked() {
                        eco.iter_mut().for_each(|(on, _)| *on = false);
                    }
                });
            });
            if !open {
                self.pcb.eco = None;
            }
        }
        let nets: Vec<String> =
            self.pcb.layout().map(|l| l.nets.iter().cloned().collect()).unwrap_or_default();
        if let Some((net, spacing)) = &mut self.pcb.stitching {
            let mut open = true;
            egui::Window::new("Via Stitching").open(&mut open).collapsible(false).show(ctx, |ui| {
                ui.label("Vias through the board on a grid, wherever one fits inside the net's pours and clear of everything else: they join the pours on every layer.");
                ui.horizontal(|ui| {
                    ui.label("Net");
                    egui::ComboBox::from_id_salt("stitch-net").selected_text(net.clone()).show_ui(ui, |ui| {
                        for n in &nets {
                            ui.selectable_value(net, n.clone(), n);
                        }
                    });
                    ui.label("every");
                    ui.add(egui::DragValue::new(spacing).range(1.0..=50.0).speed(0.1).suffix(" mm"));
                });
                if ui.button(RichText::new("Add Stitching Vias").strong()).clicked() {
                    run = Some("stitch");
                }
            });
            if !open {
                self.pcb.stitching = None;
            }
        }
        let mut widen: Option<String> = None;
        // The analysis follows the board as it's edited.
        let checked_now =
            self.pcb.analysis.as_ref().filter(|a| a.checked.is_some()).map(|a| a.revision);
        if let (Some(pw), Some(rev)) = (&self.pcb.power, checked_now) {
            if pw.revision != rev && pw.why.is_none() {
                self.pcb_power();
            }
        }
        let refs: Vec<String> = self
            .pcb
            .layout()
            .map(|l| {
                l.pads
                    .iter()
                    .map(|pd| format!("{}.{}", l.parts[pd.part].reference, pd.number))
                    .collect()
            })
            .unwrap_or_default();
        let thickness = self.pcb.layout().map_or(0.035, circuiter_pcb::power::copper_thickness);
        if let Some(pw) = &mut self.pcb.power {
            let mut open = true;
            egui::Window::new("Power Analysis").open(&mut open).collapsible(false).resizable(true).default_width(460.0).show(ctx, |ui| {
                ui.label("Each net's copper as resistors (tracks by length, width and copper weight; vias by their barrels; pours as sheets), carrying the currents the simulated operating point draws through each pad.");
                ui.add_space(4.0);
                if let Some(why) = &pw.why {
                    ui.colored_label(Color32::from_rgb(230, 170, 60), why);
                }
                if !pw.nets.is_empty() {
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        egui::Grid::new("power-nets").striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
                            for h in ["Net", "Worst drop", "At", "Peak", "Over IPC-2221"] {
                                ui.label(RichText::new(h).strong());
                            }
                            ui.end_row();
                            for n in &pw.nets {
                                let shown = pw.shown.as_ref() == Some(&n.net);
                                if ui.selectable_label(shown, &n.net).on_hover_text("Colour its tracks by load").clicked() {
                                    pw.shown = Some(n.net.clone());
                                }
                                ui.label(format!("{:.2} mV", n.worst_drop * 1e3));
                                ui.label(n.worst_pad.and_then(|k| refs.get(k).cloned()).unwrap_or_default());
                                ui.label(format!("{:.3} A", n.peak));
                                if n.overloaded.is_empty() {
                                    ui.label("–");
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(Color32::from_rgb(235, 70, 60), format!("{} track(s)", n.overloaded.len()));
                                        if ui.small_button("Widen").on_hover_text("Widen them to carry their current").clicked() {
                                            widen = Some(n.net.clone());
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        for (label, load) in [("cool", 0.0), ("half", 0.6), ("near limit", 0.84), ("over", 1.2)] {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                            ui.painter().rect_filled(r, 2.0, heat(load));
                            ui.label(RichText::new(label).small());
                        }
                    });
                }
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new("Track width for a current (IPC-2221)").strong());
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut pw.amps).range(0.001..=100.0).speed(0.01).suffix(" A"));
                    ui.label("rising");
                    ui.add(egui::DragValue::new(&mut pw.rise).range(1.0..=100.0).speed(0.5).suffix(" °C"));
                });
                let outer = circuiter_pcb::power::width_for_current(pw.amps, thickness, pw.rise, true);
                let inner = circuiter_pcb::power::width_for_current(pw.amps, thickness, pw.rise, false);
                ui.label(format!("{outer:.3} mm on an outer layer, {inner:.3} mm inside ({:.0} µm copper)", thickness * 1e3));
                ui.add_space(4.0);
                if ui.button(RichText::new("Analyse").strong()).on_hover_text("Again, with the latest operating point and board").clicked() {
                    run = Some("power");
                }
            });
            if !open {
                self.pcb.power = None;
            }
        }
        if let Some(net) = widen {
            self.pcb_widen(&net);
        }
        let selected = match (&self.pcb.selected, self.pcb.layout()) {
            (Some(Pick::Track(k)), Some(l)) => l
                .board
                .tracks
                .get(*k)
                .map(|t| (t.net.clone(), circuiter_pcb::nets::routed_length(&l.board, &t.net))),
            _ => None,
        };
        if let Some((target, amplitude, space)) = &mut self.pcb.tuning {
            let mut open = true;
            egui::Window::new("Length Tuning").open(&mut open).collapsible(false).show(ctx, |ui| {
                match &selected {
                    Some((net, now)) => {
                        ui.label(format!("{net} is routed {now:.2} mm. The selected straight segment gets an accordion of mitered bumps adding the rest."));
                        egui::Grid::new("tune").num_columns(2).show(ui, |ui| {
                            ui.label("Target length");
                            ui.add(egui::DragValue::new(target).range(0.0..=10000.0).speed(0.1).suffix(" mm"));
                            ui.end_row();
                            ui.label("Tallest bump");
                            ui.add(egui::DragValue::new(amplitude).range(0.2..=20.0).speed(0.05).suffix(" mm"));
                            ui.end_row();
                            ui.label("Bump spacing");
                            ui.add(egui::DragValue::new(space).range(0.2..=10.0).speed(0.05).suffix(" mm"));
                            ui.end_row();
                        });
                        if ui.button(RichText::new("Tune").strong()).clicked() {
                            run = Some("tune");
                        }
                    }
                    None => {
                        ui.label("Select a straight track segment of the net to tune.");
                    }
                }
            });
            if !open {
                self.pcb.tuning = None;
            }
        }
        match run {
            Some("power") => self.pcb_power(),
            Some("eco") => {
                let chosen: Vec<_> = self
                    .pcb
                    .eco
                    .take()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, c)| c)
                    .collect();
                let Some(ws) = &mut self.workspace else { return };
                match ws.edit_board(|design, id, b| {
                    sync::apply(design, id, b.as_mut().ok_or("no board")?, &chosen)
                }) {
                    Ok(_) => {
                        self.pcb.note =
                            Some(format!("Made {} change(s) from the schematic", chosen.len()))
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Some("stitch") => {
                let Some((net, spacing)) = self.pcb.stitching.clone() else { return };
                let Some(a) = &self.pcb.analysis else { return };
                let (Ok(layout), Some(checked)) = (&a.layout, &a.checked) else { return };
                let vias = circuiter_pcb::teardrops::stitch(layout, &checked.fills, &net, spacing);
                if vias.is_empty() {
                    self.pcb.note = Some(format!(
                        "No room for stitching vias: {net} needs a pour (zone) for them to sit in"
                    ));
                    return;
                }
                let n = vias.len();
                board_edit(self, |b| b.vias.extend(vias));
                self.pcb.note = Some(format!("Added {n} stitching via(s) to {net}"));
                self.pcb.stitching = None;
            }
            Some("tune") => {
                let Some((target, amplitude, space)) = self.pcb.tuning else { return };
                let (Some(Pick::Track(k)), Some(layout)) =
                    (self.pcb.selected.clone(), self.pcb.layout().cloned())
                else {
                    return;
                };
                let Some(t) = layout.board.tracks.get(k).cloned() else { return };
                let extra = target - circuiter_pcb::nets::routed_length(&layout.board, &t.net);
                let mut board = layout.board.clone();
                let mut router = self.pcb.router();
                let mut clear = |pts: &[P]| match router.as_deref_mut() {
                    Some(r) => pts.windows(2).all(|w| r.clear(&t.net, t.layer, w[0], w[1])),
                    None => true,
                };
                match circuiter_pcb::edit::tune_length(
                    &mut board, k, extra, amplitude, space, &mut clear,
                ) {
                    Ok(added) => {
                        board_edit(self, |b| *b = board);
                        self.pcb.note = Some(format!("Added {added:.2} mm to {}", t.net));
                        self.pcb.selected = None;
                        self.pcb.tuning = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            _ => {}
        }
    }

    /// Routes a differential pair, taking off what was routed of its nets first.
    pub(super) fn pcb_route_pair(&mut self, pair: &circuiter_model::board::DiffPair) {
        let Some(layout) = self.pcb.layout().cloned() else { return };
        let mut stripped = layout.board.clone();
        circuiter_pcb::edit::unroute(&mut stripped, Some(&pair.positive));
        circuiter_pcb::edit::unroute(&mut stripped, Some(&pair.negative));
        let Some(ws) = &self.workspace else { return };
        let id = ws.current().cell.clone();
        let fresh = match Layout::new(&ws.design, &id, &stripped) {
            Ok(l) => l,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let opts = RouteOptions { pitch: 0.1, ..RouteOptions::default() };
        match circuiter_pcb::route::route_pair(&fresh, pair, &opts) {
            Ok(tracks) => {
                let n = tracks.len();
                board_edit(self, |b| {
                    *b = stripped;
                    b.tracks.extend(tracks);
                });
                self.pcb.note = Some(format!("Routed {} as a pair ({n} segments)", pair.name));
            }
            Err(e) => self.pcb.note = Some(format!("Couldn't route {}: {e}", pair.name)),
        }
    }

    /// Runs the power analysis on the current operating point.
    fn pcb_power(&mut self) {
        let Some(pw) = &mut self.pcb.power else { return };
        pw.nets.clear();
        pw.why = None;
        let Some(ws) = &self.workspace else { return };
        let Some(a) = &self.pcb.analysis else { return };
        let (Ok(layout), Some(checked)) = (&a.layout, &a.checked) else {
            pw.why = Some("The board has to be checked first".into());
            return;
        };
        pw.revision = a.revision;
        let sim = ws.fresh_sim().filter(|s| {
            s.top == a.cell && matches!(s.results, Some(crate::simulation::Results::Op(_)))
        });
        let Some(sim) = sim else {
            pw.why = Some(
                "Needs the circuit's operating point: run Simulate → Operating Point (F9) on this cell, then Analyse."
                    .into(),
            );
            return;
        };
        let currents =
            circuiter_pcb::power::pad_currents(layout, |path| sim.element_op(path).map(|(c, _)| c));
        pw.nets = circuiter_pcb::power::analyse(layout, &checked.fills, &currents);
        if pw.nets.is_empty() {
            pw.why =
                Some("No current flows through the board's parts at the operating point.".into());
        }
        if !pw.shown.as_ref().is_some_and(|s| pw.nets.iter().any(|n| &n.net == s)) {
            pw.shown =
                pw.nets.iter().max_by(|a, b| a.peak.total_cmp(&b.peak)).map(|n| n.net.clone());
        }
    }

    /// Widens a net's tracks that carry more than IPC-2221 allows, to what they need
    /// (rounded up to 0.05 mm, kept clear of other nets where possible).
    fn pcb_widen(&mut self, net: &str) {
        let Some(pw) = &self.pcb.power else { return };
        let Some(layout) = self.pcb.layout() else { return };
        let Some(np) = pw.nets.iter().find(|n| n.net == net) else { return };
        let t = circuiter_pcb::power::copper_thickness(layout);
        let rise = pw.rise;
        let widths: Vec<(usize, f64)> = np
            .tracks
            .iter()
            .filter(|(k, _)| np.overloaded.contains(k))
            .filter_map(|&(k, amps)| {
                let tr = layout.board.tracks.get(k)?;
                let outer = matches!(tr.layer, Layer::FCu | Layer::BCu);
                let w = circuiter_pcb::power::width_for_current(amps * 1.1, t, rise, outer);
                Some((k, (w / 0.05).ceil() * 0.05))
            })
            .collect();
        let n = widths.len();
        board_edit(self, |b| {
            for (k, w) in widths {
                if let Some(tr) = b.tracks.get_mut(k) {
                    tr.width = Length::from_mm(w.max(tr.width.mm()));
                }
            }
        });
        self.pcb.note =
            Some(format!("Widened {n} track(s) of {net}; run the checks to see they still clear"));
    }

    /// Rearranges the parts to shorten their connections (taking the routing off first:
    /// it'd no longer fit).
    fn pcb_optimise(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let mut result = (0.0, 0.0);
        let mut unrouted = false;
        let done = ws.edit_board(|design, id, b| {
            let b = b.as_mut().ok_or("no board")?;
            if !b.tracks.is_empty() || !b.vias.is_empty() {
                circuiter_pcb::edit::unroute(b, None);
                unrouted = true;
            }
            result = sync::optimize(design, id, b)?;
            Ok(())
        });
        match done {
            Ok(()) => {
                let (before, after) = result;
                let saved = if before > 0.0 { (1.0 - after / before) * 100.0 } else { 0.0 };
                self.pcb.note = Some(format!(
                    "Placement optimised: connections {saved:.0}% shorter ({before:.0} → {after:.0} mm){}",
                    if unrouted {
                        "; the old routing was taken off, Autoroute to route again"
                    } else {
                        ""
                    }
                ));
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn pcb_toolbar(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let mut action: Option<&'static str> = None;
        let mut align_how: Option<circuiter_pcb::edit::Align> = None;
        let editable = self.workspace.as_ref().is_some_and(|ws| ws.editable());
        let has_board = self.pcb.analysis.as_ref().is_some_and(|a| a.layout.is_ok());
        let nets: Vec<String> =
            self.pcb.layout().map(|l| l.nets.iter().cloned().collect()).unwrap_or_default();
        let copper: Vec<Layer> = self.pcb.layout().map(|l| l.board.copper()).unwrap_or_default();
        let has_selection = self.pcb_selected_nets().is_some();
        egui::Frame::new().fill(p.raised).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let view = &mut self.pcb;
                if ui.button(format!("{}  Fit", icon::CORNERS_OUT)).on_hover_text("Home, or double-click").clicked() {
                    view.camera = None;
                }
                ui.separator();
                let current = view.tool.name();
                let tool = |ui: &mut egui::Ui, glyph: &str, name: &str, tip: &str| {
                    style::icon_button(ui, glyph, tip, editable && has_board, current == name).clicked()
                };
                if tool(ui, icon::CURSOR, "Select", "Select and move  (Esc)\nDrag parts, tracks, corners and vias · R turn · F flip · L lock · Del delete") {
                    view.tool = PcbTool::Select;
                }
                if tool(ui, icon::PATH, "Route", "Route tracks  (X)\nClick a pad to start, click to add corners, click a pad of the same net to finish; Ctrl+click finishes it automatically.\nShift+R: push and shove / walkaround / stop at obstacle / ignore · Shift+Space: 45° / 90° / any-angle corners · Space: flip the corner · 3: track width · V: via · Backspace: last leg · Esc: stop") {
                    view.tool = PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
                }
                if tool(ui, icon::POLYGON, "Zone", "Copper pour: click its corners, double-click to finish") {
                    view.tool = PcbTool::Zone { points: Vec::new() };
                }
                if tool(ui, icon::BOUNDING_BOX, "Outline", "Board edge: click its corners, double-click to finish") {
                    view.tool = PcbTool::Outline { points: Vec::new() };
                }
                if tool(ui, icon::PROHIBIT, "Keepout", "Keepout: an area kept clear of tracks, vias, pours or parts; click its corners, double-click to finish") {
                    view.tool = PcbTool::Keepout { points: Vec::new() };
                }
                if tool(ui, icon::SELECTION_INVERSE, "Cutout", "Cutout: a hole of any shape through the board; click its corners, double-click to finish") {
                    view.tool = PcbTool::Cutout { points: Vec::new() };
                }
                if tool(ui, icon::CIRCLE_DASHED, "Hole", "Mounting hole: click to drill a round hole through the board") {
                    view.tool = PcbTool::Hole;
                }
                if view.tool == PcbTool::Hole {
                    ui.add(egui::DragValue::new(&mut view.hole).range(0.3..=50.0).speed(0.1).suffix(" mm"))
                        .on_hover_text("The hole's diameter: 3.2 mm takes an M3 screw");
                }
                ui.separator();
                if style::icon_button(ui, icon::STACK, "Layers panel: show, hide and pick layers", true, view.show_layers).clicked() {
                    view.show_layers = !view.show_layers;
                }
                // The copper layers, one chip each: click to work on it; while routing,
                // a via takes the track there.
                let routing = matches!(&view.tool, PcbTool::Route { points, .. } if !points.is_empty());
                for &l in &copper {
                    let on = view.layer == l;
                    let c = layer_colour(l);
                    let chip = egui::Button::new(RichText::new(l.name()).color(if on { Color32::BLACK } else { c }).strong())
                        .fill(if on { c } else { c.gamma_multiply(0.15) })
                        .stroke(Stroke::new(1.0, c.gamma_multiply(0.8)))
                        .corner_radius(4);
                    let tip = if routing {
                        format!("Drop a via and carry on routing on {}", l.name())
                    } else {
                        format!("Work on {}  (PgUp / PgDn)", l.name())
                    };
                    if ui.add(chip).on_hover_text(tip).clicked() && !on {
                        if routing {
                            view.pending_layer = Some(l);
                        } else {
                            view.layer = l;
                        }
                        view.visible.hidden.remove(&l);
                    }
                }
                if routing {
                    let other = if view.layer == Layer::FCu { *copper.last().unwrap_or(&Layer::BCu) } else { Layer::FCu };
                    if ui.button(format!("{}  Via", icon::ARROWS_DOWN_UP)).on_hover_text(format!("Drop a via here and route on to {}  (V)", other.name())).clicked() {
                        view.pending_layer = Some(other);
                    }
                }
                // How the other layers show: all, dimmed, or hidden.
                let isolation = if view.single_layer { 2 } else { usize::from(view.visible.high_contrast) };
                if let Some(k) = style::segmented(ui, &[("All", true), ("Dim", true), ("Only", true)], isolation) {
                    view.single_layer = k == 2;
                    view.visible.high_contrast = k == 1;
                }
                ui.label(RichText::new("other layers").small().color(p.weak));
                if style::icon_button(ui, icon::CIRCUITRY, "Realistic view: solder mask, gold pads and white legend, as the board house makes it", true, view.visible.realistic).clicked() {
                    view.visible.realistic = !view.visible.realistic;
                }
                if matches!(view.tool, PcbTool::Zone { .. }) {
                    if view.zone_net.is_empty() || !nets.contains(&view.zone_net) {
                        view.zone_net = nets.iter().find(|n| n.eq_ignore_ascii_case("gnd") || *n == "0").or(nets.first()).cloned().unwrap_or_default();
                    }
                    egui::ComboBox::from_id_salt("zone-net").selected_text(format!("Net: {}", view.zone_net)).show_ui(ui, |ui| {
                        for n in &nets {
                            ui.selectable_value(&mut view.zone_net, n.clone(), n);
                        }
                    });
                }
                egui::ComboBox::from_id_salt("pcb-grid").selected_text(format!("Grid {} mm", view.grid)).show_ui(ui, |ui| {
                    for &g in GRIDS {
                        ui.selectable_value(&mut view.grid, g, format!("{g} mm"));
                    }
                });
                ui.separator();
                let can = editable && has_board;
                let teardrops = self.pcb.layout().is_some_and(|l| l.board.teardrops.is_some());
                let track_selected = matches!(self.pcb.selected, Some(Pick::Track(_)));
                let group_size = self.pcb.group.len();
                ui.menu_button(format!("{}  Tools", icon::GEAR_SIX), |ui| {
                    if ui.add_enabled(can, egui::Button::new("Optimise Placement")).on_hover_text("Move and turn parts to shorten their connections (unlocked parts only)").clicked() {
                        action = Some("optimise");
                        ui.close();
                    }
                    ui.add_enabled_ui(can && group_size >= 2, |ui| {
                        ui.menu_button("Align", |ui| {
                            use circuiter_pcb::edit::Align as A;
                            for (name, how) in [
                                ("Left Edges", A::Left),
                                ("Right Edges", A::Right),
                                ("Top Edges", A::Top),
                                ("Bottom Edges", A::Bottom),
                                ("Centres on a Vertical Line", A::CentreX),
                                ("Centres on a Horizontal Line", A::CentreY),
                                ("Distribute Across", A::SpreadX),
                                ("Distribute Down", A::SpreadY),
                            ] {
                                if ui.button(name).clicked() {
                                    align_how = Some(how);
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text("Select parts first: drag a box round them, or Shift+click");
                    });
                    ui.separator();
                    if ui.add_enabled(can, egui::Button::new(if teardrops { "Remove Teardrops" } else { "Add Teardrops" })).on_hover_text("Widen tracks where they meet pads and vias").clicked() {
                        action = Some("teardrops");
                        ui.close();
                    }
                    if ui.add_enabled(can, egui::Button::new("Via Stitching…")).clicked() {
                        action = Some("stitching");
                        ui.close();
                    }
                    if ui.add_enabled(can && track_selected, egui::Button::new("Tune Length…")).on_hover_text("Select a straight segment of the net first").clicked() {
                        action = Some("tuning");
                        ui.close();
                    }
                    if ui.add_enabled(can, egui::Button::new("Power Analysis…")).on_hover_text("Current in each track and voltage lost across each power net, from the simulated operating point").clicked() {
                        action = Some("power");
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Measure   Ctrl+M").clicked() {
                        action = Some("measure");
                        ui.close();
                    }
                });
                ui.menu_button(format!("{}  Autoroute", icon::FLOW_ARROW), |ui| {
                    if ui.add_enabled(can, egui::Button::new("Route All")).on_hover_text("Turn every ratsnest line into tracks: shortest first, at 45°, with vias where it must change layer").clicked() {
                        action = Some("route-all");
                        ui.close();
                    }
                    if ui.add_enabled(can && has_selection, egui::Button::new("Route the Selection's Nets")).on_hover_text("Select a part, track or via first").clicked() {
                        action = Some("route-net");
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(can, egui::Button::new("Clean Up Tracks")).on_hover_text("Join segments in a line, take off stubs and duplicates").clicked() {
                        action = Some("clean");
                        ui.close();
                    }
                    if ui.add_enabled(can && has_selection, egui::Button::new("Unroute the Selection's Nets")).clicked() {
                        action = Some("unroute-net");
                        ui.close();
                    }
                    if ui.add_enabled(can, egui::Button::new("Unroute All")).clicked() {
                        action = Some("unroute-all");
                        ui.close();
                    }
                });
                ui.separator();
                if ui.add_enabled(editable && has_board, egui::Button::new(format!("{}  Update from Schematic", icon::ARROWS_CLOCKWISE)))
                    .on_hover_text("Place parts new to the schematic and take off those deleted from it")
                    .clicked()
                {
                    action = Some("update");
                }
                if ui.add_enabled(has_board, egui::Button::new(format!("{}  Fabrication Files…", icon::EXPORT)))
                    .on_hover_text("Gerbers, drill files, positions and bill of materials, for the board house")
                    .clicked()
                {
                    action = Some("fab");
                }
                if ui.add_enabled(has_board, egui::Button::new(format!("{}  3D", icon::CUBE))).on_hover_text("See the board with its parts in 3D").clicked() {
                    action = Some("3d");
                }
                if let Some(v) = self.pcb.layout().and_then(|l| l.board.active_variant()) {
                    ui.label(RichText::new(format!("{}  {}", icon::STACK, v.name)).color(style::palette(ui).accent))
                        .on_hover_text("The assembly variant being built: change it in the board's properties");
                }
            });
        });
        if let (Some(how), Some(layout)) = (align_how, self.pcb.layout().cloned()) {
            let paths: Vec<String> = self.pcb.group.iter().cloned().collect();
            board_edit(self, |b| circuiter_pcb::edit::align(b, &layout, &paths, how));
        }
        match action {
            Some("route-all") => self.pcb_autoroute(None),
            Some("route-net") => {
                let nets = self.pcb_selected_nets();
                if nets.is_some() {
                    self.pcb_autoroute(nets);
                }
            }
            Some(a @ ("clean" | "unroute-net" | "unroute-all")) => self.pcb_tracks(a),
            Some("optimise") => self.pcb_optimise(),
            Some("teardrops") => {
                board_edit(self, |b| {
                    b.teardrops =
                        if b.teardrops.is_some() { None } else { Some(Default::default()) }
                });
            }
            Some("stitching") => {
                let net =
                    self.pcb.layout().and_then(|l| {
                        l.board.zones.first().map(|z| z.net.clone()).or_else(|| {
                            l.nets.iter().find(|n| n.eq_ignore_ascii_case("gnd")).cloned()
                        })
                    });
                self.pcb.stitching = Some((net.unwrap_or_default(), 5.0));
            }
            Some("tuning") => {
                let start = match (&self.pcb.selected, self.pcb.layout()) {
                    (Some(Pick::Track(k)), Some(l)) => l.board.tracks.get(*k).map(|t| {
                        let now = circuiter_pcb::nets::routed_length(&l.board, &t.net);
                        l.board
                            .rules
                            .class(&t.net)
                            .and_then(|c| c.min_length)
                            .map_or(now + 5.0, |m| m.mm())
                    }),
                    _ => None,
                };
                self.pcb.tuning = Some((start.unwrap_or(10.0), 1.5, 1.0));
            }
            Some("measure") => self.pcb.tool = PcbTool::Measure { from: None, to: None },
            Some("power") => {
                self.pcb.power = Some(PowerView::default());
                self.pcb_power();
            }
            Some("update") => self.pcb_update_from_schematic(),
            Some("fab") => self.pcb_export(),
            Some("3d") => {
                self.show_pcb = false;
                self.show_3d = true;
                self.viewer3d.body_mode = false;
                self.viewer3d.board = true;
            }
            _ => {}
        }
    }

    /// Writes the fabrication files into a folder the user picks.
    pub(super) fn pcb_export(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let Some(layout) = self.pcb.layout() else { return };
        let Some(checked) = self.pcb.analysis.as_ref().and_then(|a| a.checked.as_ref()) else {
            return;
        };
        let name = ws.current().cell.cell.clone();
        let default = ws.project.root.join("fab").join(&name);
        let dir = if cfg!(test) {
            Some(default)
        } else {
            rfd::FileDialog::new().set_directory(&ws.project.root).pick_folder()
        };
        let Some(dir) = dir else { return };
        let mut files = fab::outputs(layout, &checked.fills, &name);
        // Each variant's bill of materials and placement file.
        for v in &layout.board.variants {
            let mut b = layout.board.clone();
            b.variant = Some(v.name.clone());
            if let Ok(l) = Layout::new(&ws.design, &ws.current().cell, &b) {
                let tag = v.name.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
                files.push(fab::Output {
                    name: format!("{name}-{tag}-bom.csv"),
                    contents: fab::bom(&l),
                });
                files.push(fab::Output {
                    name: format!("{name}-{tag}-pos.csv"),
                    contents: fab::positions(&l),
                });
            }
        }
        // The panel's files beside the board's, with a picture of it.
        let mut panel_error = None;
        if let Some(pn) = &layout.board.panel {
            match circuiter_pcb::panel::panelize(layout, &checked.fills, pn) {
                Ok(made) => {
                    files.extend(fab::panel_outputs(&made, &name));
                    files.push(fab::Output {
                        name: format!("{name}-panel.svg"),
                        contents: circuiter_pcb::svg::plot_panel(&made),
                    });
                }
                Err(e) => panel_error = Some(e),
            }
        }
        let mut svg_path = dir.join(format!("{name}.svg"));
        let svg = circuiter_pcb::svg::plot(layout, &checked.fills, Some(&checked.connectivity));
        let written = fab::write(&dir, &files).and_then(|()| std::fs::write(&svg_path, svg));
        if written.is_err() {
            svg_path = dir.clone();
        }
        match written {
            Ok(()) => {
                let errors = checked.errors();
                let mut warn = if errors > 0 {
                    format!(" — the board still breaks {errors} design rule(s)")
                } else {
                    String::new()
                };
                if let Some(e) = &panel_error {
                    warn.push_str(&format!(" — no panel: {e}"));
                }
                self.pcb.note =
                    Some(format!("Wrote {} files to {}{warn}", files.len() + 1, dir.display()));
            }
            Err(e) => self.error = Some(format!("writing {}: {e}", svg_path.display())),
        }
    }

    /// Pointer and keys over the board.
    fn pcb_interact(&mut self, ui: &mut egui::Ui, rect: egui::Rect, response: &egui::Response) {
        let Some(layout) = self.pcb.layout() else { return };
        // Fit to the board the first time and whenever asked.
        if self.pcb.camera.is_none() {
            let outline: Vec<P> = layout.board.outline.iter().map(|q| p(*q)).collect();
            let mut pts = outline.clone();
            for part in &layout.parts {
                if let Some(place) = part.place {
                    pts.push(place.at);
                }
            }
            let (lo, hi) = pts.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
                ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
            });
            let bounds = if lo[0] <= hi[0] {
                circuiter_scene::Rect { min: Pt::new(lo[0], lo[1]), max: Pt::new(hi[0], hi[1]) }
            } else {
                circuiter_scene::Rect { min: Pt::new(0.0, 0.0), max: Pt::new(50.0, 40.0) }
            };
            let mut cam = Camera::fit(&bounds, rect);
            cam.fitted = None;
            self.pcb.camera = Some(cam);
        }
        let mut cam = self.pcb.camera.expect("fitted");
        // Navigation: wheel zooms about the pointer; middle or right drag pans.
        // Ctrl+Shift+wheel steps through the copper layers, as in Altium.
        let (ctrl_shift, wheel_step) = ui.input(|i| {
            (
                i.modifiers.command && i.modifiers.shift,
                i.smooth_scroll_delta.y + i.smooth_scroll_delta.x,
            )
        });
        if response.hovered() && !ctrl_shift {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if let (true, Some(at)) = (scroll != 0.0, response.hover_pos()) {
                cam.zoom_about(rect, at, (scroll * 0.002).exp());
            }
        }
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
            || (space && response.dragged_by(egui::PointerButton::Primary))
        {
            let d = response.drag_delta() / cam.scale;
            cam.center = Pt::new(cam.center.x - f64::from(d.x), cam.center.y - f64::from(d.y));
        }
        self.pcb.camera = Some(cam);
        self.pcb.rect = Some(rect);
        let world = |pos: Pos2| {
            let w = cam.to_world(rect, pos);
            [w.x, w.y]
        };
        let tolerance = f64::from(4.0 / cam.scale);
        let hover = response.hover_pos().map(world);
        self.pcb.cursor = hover;
        let editable = self.workspace.as_ref().is_some_and(|ws| ws.editable());
        let typing = ui.ctx().egui_wants_keyboard_input();
        let keys = |k: egui::Key| !typing && ui.input(|i| i.key_pressed(k));
        let (shift, cmd) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        // Double-clicking the board's edge adds a corner there.
        let on_edge = (self.pcb.tool == PcbTool::Select && editable && response.double_clicked())
            .then_some(hover)
            .flatten()
            .and_then(|at| {
                let b = &self.pcb.layout()?.board;
                let k = edge_near(b, at, tolerance)?;
                edge_handle(b, at, tolerance).is_none().then_some((k, at))
            });
        if keys(egui::Key::Home)
            || (response.double_clicked()
                && self.pcb.tool == PcbTool::Select
                && self.pcb.selected.is_none()
                && on_edge.is_none())
        {
            self.pcb.camera = None;
        }
        if !editable {
            return;
        }
        let layer = self.pcb.layer;
        let mut edit: Option<BoardEdit> = None;
        let layout = self.pcb.layout().expect("board").clone();
        if let Some((k, at)) = on_edge {
            let at = self.pcb.snap(at);
            edit = Some(Box::new(move |b: &mut Board| {
                b.split_edge(k, point(at));
                Ok(())
            }));
            self.pcb.selected = Some(Pick::Corner(k + 1));
        }
        match self.pcb.tool.clone() {
            PcbTool::Select => {
                if response.drag_started_by(egui::PointerButton::Primary) && !space {
                    // From where the button went down, not where the drag was noticed.
                    let origin =
                        ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
                    if let Some(from) = origin.map(world) {
                        let handle = edge_handle(&layout.board, from, tolerance);
                        let picked = match &handle {
                            Some(Drag::EdgeCorner(k)) => Some(Pick::Corner(*k)),
                            _ => pick(&layout, from, tolerance, layer),
                        };
                        let drag = match &picked {
                            _ if handle.is_some() => handle,
                            // One of a group: the group moves.
                            Some(Pick::Footprint(path))
                                if self.pcb.group.len() > 1 && self.pcb.group.contains(path) =>
                            {
                                let paths: Vec<String> = self
                                    .pcb
                                    .group
                                    .iter()
                                    .filter(|p| !layout.board.placed(p).is_some_and(|f| f.locked))
                                    .cloned()
                                    .collect();
                                Some(Drag::Footprints(paths))
                            }
                            Some(Pick::Footprint(path)) => {
                                (!layout.board.placed(path).is_some_and(|f| f.locked))
                                    .then(|| Drag::Footprint(path.clone()))
                            }
                            Some(Pick::Track(k)) => {
                                let t = &layout.board.tracks[*k];
                                // Near an end that isn't on a pad: drag the corner.
                                let grab = (t.width.mm()).max(tolerance * 2.0);
                                let end = [p(t.a), p(t.b)]
                                    .into_iter()
                                    .find(|e| (e[0] - from[0]).hypot(e[1] - from[1]) < grab);
                                let on_pad = |e: P| {
                                    layout.pads.iter().any(|pd| {
                                        (pd.center[0] - e[0]).hypot(pd.center[1] - e[1]) < 1e-6
                                    })
                                };
                                match end {
                                    Some(e) if !on_pad(e) => Some(Drag::Corner(t.layer, e)),
                                    _ => Some(Drag::Segment(*k)),
                                }
                            }
                            Some(Pick::Via(k)) => Some(Drag::Via(*k)),
                            Some(Pick::Cutout(k)) => Some(Drag::Cutout(*k)),
                            _ => None,
                        };
                        // Empty board (or a pour): a selection box.
                        let open =
                            matches!(picked, None | Some(Pick::Zone(_)) | Some(Pick::Keepout(_)));
                        if open {
                            self.pcb.band = Some((from, from));
                        }
                        if !open && !matches!(drag, Some(Drag::Footprints(_))) {
                            self.pcb.selected = picked;
                        }
                        self.pcb.drag = drag.map(|d| (d, from, from));
                    }
                }
                if let (Some(drag), Some(to)) =
                    (&mut self.pcb.drag, response.interact_pointer_pos().map(world))
                {
                    drag.2 = to;
                }
                if let (Some(band), Some(to)) =
                    (&mut self.pcb.band, response.interact_pointer_pos().map(world))
                {
                    band.1 = to;
                }
                if response.drag_stopped() {
                    if let Some((a, b)) = self.pcb.band.take() {
                        let (lo, hi) =
                            ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
                        let inside: Vec<String> = layout
                            .parts
                            .iter()
                            .filter(|part| {
                                part.place.is_some_and(|pl| {
                                    pl.at[0] >= lo[0]
                                        && pl.at[0] <= hi[0]
                                        && pl.at[1] >= lo[1]
                                        && pl.at[1] <= hi[1]
                                })
                            })
                            .map(|part| part.path.clone())
                            .collect();
                        if !shift {
                            self.pcb.group.clear();
                        }
                        self.pcb.group.extend(inside);
                        self.pcb.selected = None;
                        self.pcb.note = Some(format!("{} part(s) selected", self.pcb.group.len()));
                    }
                }
                self.pcb.preview = self.pcb.drag.as_ref().map(|(d, from, to)| {
                    apply_drag(&layout.board, &layout, d, *from, *to, self.pcb.grid)
                });
                if response.drag_stopped() {
                    self.pcb.preview = None;
                    if let Some((d, from, to)) = self.pcb.drag.take() {
                        let grid = self.pcb.grid;
                        if (to[0] - from[0]).hypot(to[1] - from[1]) > 1e-6 {
                            let layout = layout.clone();
                            edit = Some(Box::new(move |b: &mut Board| {
                                *b = apply_drag(b, &layout, &d, from, to, grid);
                                Ok(())
                            }));
                        }
                    }
                }
                if response.clicked_by(egui::PointerButton::Primary) {
                    let picked =
                        hover.and_then(|at| match edge_handle(&layout.board, at, tolerance) {
                            Some(Drag::EdgeCorner(k)) => Some(Pick::Corner(k)),
                            _ => pick(&layout, at, tolerance, layer),
                        });
                    // Shift+click adds a part to the group, or takes it out.
                    match (&picked, shift) {
                        (Some(Pick::Footprint(path)), true) => {
                            if let Some(Pick::Footprint(first)) = &self.pcb.selected {
                                self.pcb.group.insert(first.clone());
                            }
                            if !self.pcb.group.remove(path) {
                                self.pcb.group.insert(path.clone());
                            }
                        }
                        _ => self.pcb.group.clear(),
                    }
                    self.pcb.selected = picked;
                }
                // R turns every part of a group, each about itself.
                if self.pcb.group.len() > 1 && keys(egui::Key::R) && !cmd {
                    let (paths, layout) = (self.pcb.group.clone(), layout.clone());
                    edit = Some(Box::new(move |b: &mut Board| {
                        for path in &paths {
                            if let Some(f) = b.placed(path).filter(|f| !f.locked).cloned() {
                                let mut turned = f.clone();
                                turned.rotation = (f.rotation + 90.0).rem_euclid(360.0);
                                circuiter_pcb::edit::move_footprint(b, &layout, path, turned);
                            }
                        }
                        Ok(())
                    }));
                }
                // Keys act on the selection.
                match self.pcb.selected.clone() {
                    Some(Pick::Footprint(path)) => {
                        let toggle = |k| keys(k) && !cmd;
                        let (r, f, l) =
                            (toggle(egui::Key::R), toggle(egui::Key::F), toggle(egui::Key::L));
                        let del = keys(egui::Key::Delete);
                        if r || f || l || del {
                            let layout = layout.clone();
                            edit = Some(Box::new(move |b: &mut Board| {
                                if del {
                                    b.footprints.retain(|x| x.path != path);
                                    return Ok(());
                                }
                                let mut fp = b.placed(&path).ok_or("gone")?.clone();
                                if l {
                                    fp.locked = !fp.locked;
                                } else if fp.locked {
                                    return Err(format!("{path} is locked (L to unlock)"));
                                }
                                if r {
                                    fp.rotation = (fp.rotation + if shift { -90.0 } else { 90.0 })
                                        .rem_euclid(360.0);
                                }
                                if f {
                                    fp.back = !fp.back;
                                }
                                // Its tracks come with it.
                                circuiter_pcb::edit::move_footprint(b, &layout, &path, fp);
                                Ok(())
                            }));
                            if del {
                                self.pcb.selected = None;
                            }
                        }
                    }
                    Some(
                        pick @ (Pick::Track(_)
                        | Pick::Via(_)
                        | Pick::Zone(_)
                        | Pick::Keepout(_)
                        | Pick::Corner(_)
                        | Pick::Cutout(_)),
                    ) if keys(egui::Key::Delete) || keys(egui::Key::Backspace) => {
                        self.pcb.selected = None;
                        edit = Some(Box::new(move |b: &mut Board| {
                            match pick {
                                Pick::Track(k) if k < b.tracks.len() => drop(b.tracks.remove(k)),
                                Pick::Via(k) if k < b.vias.len() => drop(b.vias.remove(k)),
                                Pick::Zone(k) if k < b.zones.len() => drop(b.zones.remove(k)),
                                Pick::Keepout(k) if k < b.keepouts.len() => {
                                    drop(b.keepouts.remove(k))
                                }
                                Pick::Corner(k) => b.remove_corner(k),
                                Pick::Cutout(k) if k < b.graphics.len() => {
                                    drop(b.graphics.remove(k))
                                }
                                _ => {}
                            }
                            Ok(())
                        }));
                    }
                    _ => {}
                }
                if keys(egui::Key::X) && !cmd {
                    self.pcb.tool =
                        PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
                }
                if keys(egui::Key::Escape) {
                    self.pcb.selected = None;
                }
            }
            PcbTool::Route { net, width, points } => {
                // Shoved tracks show only while a route is being drawn; set again below.
                self.pcb.preview = None;
                let at = hover.map(|h| match copper_at(&layout, h, tolerance, layer) {
                    Some((Some(n), q)) if n == net => q,
                    _ => self.pcb.snap(h),
                });
                let shift_down = ui.input(|i| i.modifiers.shift);
                // Altium's keys: Shift+R the conflict mode, Shift+Space the corner style,
                // Space (or /) which leg comes first, 3 the track width.
                if !typing && shift_down && ui.input(|i| i.key_pressed(egui::Key::R)) {
                    self.pcb.mode = self.pcb.mode.next();
                    self.pcb.note = Some(format!("Routing mode: {}", self.pcb.mode.name()));
                }
                if keys(egui::Key::Space) && !points.is_empty() {
                    if shift_down {
                        self.pcb.corner = self.pcb.corner.next();
                        self.pcb.note = Some(format!("Corners: {}", self.pcb.corner.name()));
                    } else {
                        self.pcb.diagonal_first = !self.pcb.diagonal_first;
                    }
                }
                if keys(egui::Key::Slash) {
                    self.pcb.diagonal_first = !self.pcb.diagonal_first;
                }
                if keys(egui::Key::Num3) && !net.is_empty() {
                    let rules = &layout.board.rules;
                    let preferred = rules.track_width_for(&net).mm();
                    let choices = [rules.min_track_width.mm(), preferred, preferred * 2.0];
                    let k = choices
                        .iter()
                        .position(|w| (w - width).abs() < 1e-9)
                        .map_or(1, |k| (k + 1) % 3);
                    self.pcb.tool = PcbTool::Route {
                        net: net.clone(),
                        width: choices[k],
                        points: points.clone(),
                    };
                    self.pcb.note = Some(format!("Track width {:.3} mm", choices[k]));
                }
                let width = match &self.pcb.tool {
                    PcbTool::Route { width, .. } => *width,
                    _ => width,
                };
                // Another layer picked mid-route: a via where the track has got to.
                if let (Some(next), Some(&head)) = (self.pcb.pending_layer.take(), points.last()) {
                    if next != layer {
                        let (diameter, drill) = layout.board.rules.via_for(&net);
                        let via = Via { at: point(head), diameter, drill, net: net.clone() };
                        self.pcb.layer = next;
                        self.pcb.note = Some(format!("Via: routing {net} on {}", next.name()));
                        edit = Some(Box::new(move |b: &mut Board| {
                            b.vias.push(via);
                            Ok(())
                        }));
                    }
                }
                let ctrl_click = response.clicked_by(egui::PointerButton::Primary) && cmd;
                if keys(egui::Key::Escape) {
                    self.pcb.route_preview = None;
                    self.pcb.tool = if points.is_empty() {
                        PcbTool::Select
                    } else {
                        PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() }
                    };
                } else if points.is_empty() {
                    self.pcb.route_preview = None;
                    if response.clicked_by(egui::PointerButton::Primary) {
                        match hover.and_then(|h| copper_at(&layout, h, tolerance, layer)) {
                            Some((Some(n), start)) => {
                                let w = layout.board.rules.track_width_for(&n).mm();
                                self.pcb.route_laid.clear();
                                if ctrl_click {
                                    // Ctrl+click: start here and finish it automatically.
                                    edit = self.pcb_auto_complete(&layout, &n, start, w);
                                } else {
                                    self.pcb.note = Some(format!(
                                        "Routing {n} on {} · {} · {} corners · Shift+R mode, Shift+Space corners, 3 width, Ctrl+click to finish automatically",
                                        layer.name(),
                                        self.pcb.mode.name(),
                                        self.pcb.corner.name()
                                    ));
                                    self.pcb.tool =
                                        PcbTool::Route { net: n, width: w, points: vec![start] };
                                }
                            }
                            Some((None, _)) => {
                                self.pcb.note = Some(
                                    "That pad isn't connected to anything in the schematic".into(),
                                )
                            }
                            None => {
                                self.pcb.note = Some(format!(
                                    "Start a route on a pad, track or via on {}",
                                    layer.name()
                                ))
                            }
                        }
                    }
                } else if ctrl_click {
                    let head = *points.last().expect("started");
                    edit = self.pcb_auto_complete(&layout, &net, head, width);
                } else if let Some(to) = at {
                    let from = *points.last().expect("started");
                    let (path, blocked) = self.pcb.route_to(&net, width, layer, from, to);
                    self.pcb.route_preview = Some((path.clone(), blocked));
                    // Tracks pushed aside show where they'll go.
                    self.pcb.preview = self.pcb.shoved.clone();
                    let shoved = self.pcb.shoved.clone();
                    let end = *path.last().expect("a start");
                    // V goes to the other side.
                    let target = keys(egui::Key::V)
                        .then(|| if layer == Layer::FCu { Layer::BCu } else { Layer::FCu });
                    let via = target.is_some();
                    let back = keys(egui::Key::Backspace);
                    if back {
                        // Take off the last leg laid.
                        let mut pts = points.clone();
                        pts.pop();
                        if pts.is_empty() {
                            self.pcb.tool = PcbTool::Route {
                                net: String::new(),
                                width: 0.0,
                                points: Vec::new(),
                            };
                        } else {
                            let undo = (pts.last().copied().expect("one"), from);
                            self.pcb.tool = PcbTool::Route { net: net.clone(), width, points: pts };
                            edit = Some(Box::new(move |b: &mut Board| {
                                if let Some(k) = b
                                    .tracks
                                    .iter()
                                    .rposition(|t| p(t.a) == undo.0 && p(t.b) == undo.1)
                                {
                                    b.tracks.remove(k);
                                }
                                Ok(())
                            }));
                        }
                    } else if (response.clicked_by(egui::PointerButton::Primary)
                        || response.double_clicked()
                        || via)
                        && path.len() >= 2
                    {
                        // Lay the legs (as far as they got); finish on copper of the net,
                        // or with a double-click.
                        let reached = (end[0] - to[0]).hypot(end[1] - to[1]) < 1e-6;
                        let ends = reached
                            && copper_at(&layout, to, tolerance, layer)
                                .is_some_and(|(n, _)| n.as_deref() == Some(net.as_str()));
                        let finish = (ends && !via) || response.double_clicked();
                        let tracks: Vec<Track> = path
                            .windows(2)
                            .map(|w| Track {
                                layer,
                                a: point(w[0]),
                                b: point(w[1]),
                                width: Length::from_mm(width),
                                net: net.clone(),
                            })
                            .collect();
                        let (dia, drill) = layout.board.rules.via_for(&net);
                        let new_via = via.then(|| Via {
                            at: point(end),
                            diameter: dia,
                            drill,
                            net: net.clone(),
                        });
                        let mut pts = points.clone();
                        pts.extend(path.iter().skip(1));
                        if let Some(other) = target {
                            self.pcb.layer = other;
                            self.pcb.note = Some(format!("Via: routing {net} on {}", other.name()));
                        }
                        self.pcb.route_laid.extend(tracks.iter().cloned());
                        let laid = self.pcb.route_laid.clone();
                        self.pcb.route_preview = None;
                        self.pcb.tool = if finish {
                            let length: f64 = laid
                                .iter()
                                .map(|t| (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm()))
                                .sum();
                            self.pcb.note = Some(format!("Routed {net}: {length:.2} mm"));
                            PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() }
                        } else {
                            PcbTool::Route { net: net.clone(), width, points: pts }
                        };
                        let layout = layout.clone();
                        let net = net.clone();
                        self.pcb.preview = None;
                        edit = Some(Box::new(move |b: &mut Board| {
                            if let Some(moved) = shoved {
                                b.tracks = moved.tracks;
                            }
                            b.tracks.extend(tracks);
                            b.vias.extend(new_via);
                            // Loop removal: a new way between two points replaces the old.
                            if finish {
                                circuiter_pcb::edit::remove_loops(b, &layout, &net, &laid);
                            }
                            Ok(())
                        }));
                    }
                }
            }
            PcbTool::Measure { from, to } => {
                if keys(egui::Key::Escape) {
                    self.pcb.tool = PcbTool::Select;
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    if let Some(h) = hover {
                        let q = self.pcb.snap(h);
                        self.pcb.tool = match (from, to) {
                            (Some(f), None) => PcbTool::Measure { from: Some(f), to: Some(q) },
                            _ => PcbTool::Measure { from: Some(q), to: None },
                        };
                    }
                }
            }
            PcbTool::Hole => {
                if keys(egui::Key::Escape) {
                    self.pcb.tool = PcbTool::Select;
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    if let Some(h) = hover {
                        let (at, r) = (point(self.pcb.snap(h)), self.pcb.hole / 2.0);
                        edit = Some(Box::new(move |b: &mut Board| {
                            b.graphics.push(Graphic::Circle {
                                layer: Layer::EdgeCuts,
                                center: at,
                                radius: Length::from_mm(r),
                                width: Length::from_mm(0.1),
                            });
                            Ok(())
                        }));
                    }
                }
            }
            PcbTool::Zone { mut points }
            | PcbTool::Outline { mut points }
            | PcbTool::Keepout { mut points }
            | PcbTool::Cutout { mut points } => {
                let kind = match self.pcb.tool {
                    PcbTool::Zone { .. } => 0,
                    PcbTool::Outline { .. } => 1,
                    PcbTool::Keepout { .. } => 2,
                    _ => 3,
                };
                let zone = kind == 0;
                let make = |points: Vec<P>| match kind {
                    0 => PcbTool::Zone { points },
                    1 => PcbTool::Outline { points },
                    2 => PcbTool::Keepout { points },
                    _ => PcbTool::Cutout { points },
                };
                if keys(egui::Key::Escape) {
                    self.pcb.tool =
                        if points.is_empty() { PcbTool::Select } else { make(Vec::new()) };
                } else if keys(egui::Key::Backspace) {
                    points.pop();
                    self.pcb.tool = make(points);
                } else if response.double_clicked() || keys(egui::Key::Enter) {
                    if points.len() >= 3 {
                        let outline: Vec<_> = points.iter().map(|&q| point(q)).collect();
                        let (net, layer) = (self.pcb.zone_net.clone(), self.pcb.layer);
                        edit = Some(if kind == 3 {
                            Box::new(move |b: &mut Board| {
                                b.graphics.push(Graphic::Poly {
                                    layer: Layer::EdgeCuts,
                                    points: outline,
                                    width: Length::from_mm(0.1),
                                    filled: false,
                                });
                                Ok(())
                            })
                        } else if kind == 2 {
                            Box::new(move |b: &mut Board| {
                                b.keepouts.push(circuiter_model::board::Keepout::new(outline));
                                Ok(())
                            })
                        } else if zone {
                            Box::new(move |b: &mut Board| {
                                if net.is_empty() {
                                    return Err("choose the pour's net first".into());
                                }
                                b.zones.push(Zone::new(layer, &net, outline));
                                Ok(())
                            })
                        } else {
                            Box::new(move |b: &mut Board| {
                                b.outline = outline;
                                b.bulges.clear();
                                Ok(())
                            })
                        });
                    }
                    self.pcb.tool = make(Vec::new());
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    if let Some(h) = hover {
                        let q = self.pcb.snap(h);
                        if points.last() != Some(&q) {
                            points.push(q);
                        }
                        self.pcb.tool = make(points);
                    }
                }
            }
        }
        if keys(egui::Key::X)
            && !cmd
            && matches!(self.pcb.tool, PcbTool::Zone { .. } | PcbTool::Outline { .. })
        {
            self.pcb.tool = PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
        }
        if !typing && cmd && ui.input(|i| i.key_pressed(egui::Key::M)) {
            self.pcb.tool = PcbTool::Measure { from: None, to: None };
        }
        // Page Up / Down (or Ctrl+Shift+wheel, or the keypad's + and −) step through
        // the copper layers; while routing, with a via.
        let copper = layout.board.copper();
        if ctrl_shift && response.hovered() {
            self.pcb.wheel += wheel_step;
        } else {
            self.pcb.wheel = 0.0;
        }
        let notch = if self.pcb.wheel.abs() >= 40.0 {
            let s = -self.pcb.wheel.signum() as i32;
            self.pcb.wheel = 0.0;
            s
        } else {
            0
        };
        let step = if keys(egui::Key::PageUp) || keys(egui::Key::Minus) || notch < 0 {
            -1
        } else if keys(egui::Key::PageDown)
            || keys(egui::Key::Plus)
            || keys(egui::Key::Equals)
            || notch > 0
        {
            1
        } else {
            0
        };
        // Shift+S: the working layer alone. F5: net colours on the routing.
        if !typing && shift && ui.input(|i| i.key_pressed(egui::Key::S)) {
            self.pcb.single_layer = !self.pcb.single_layer;
        }
        if keys(egui::Key::F5) {
            self.pcb.net_colours = !self.pcb.net_colours;
        }
        if step != 0 {
            let k = copper.iter().position(|l| *l == self.pcb.layer).unwrap_or(0) as i32;
            let next = copper[(k + step).clamp(0, copper.len() as i32 - 1) as usize];
            let routing =
                matches!(&self.pcb.tool, PcbTool::Route { points, .. } if !points.is_empty());
            if routing {
                self.pcb.pending_layer = Some(next);
            } else {
                self.pcb.layer = next;
            }
        }
        if !copper.contains(&self.pcb.layer) {
            self.pcb.layer = Layer::FCu;
        }
        if let Some(f) = edit {
            let Some(ws) = &mut self.workspace else { return };
            let done = ws.edit_board(|_, _, board| match board {
                Some(b) => f(b),
                None => Err("no board".into()),
            });
            if let Err(e) = done {
                self.error = Some(e);
            }
        }
    }

    /// Draws the board, back layer first.
    fn pcb_paint(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let view = &self.pcb;
        let Some(analysis) = &view.analysis else { return };
        let Ok(layout) = &analysis.layout else { return };
        let Some(cam) = view.camera else { return };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BACKGROUND);
        let d = Draw { painter: &painter, cam, view: rect };
        let board = &layout.board;
        let real = view.visible.realistic;
        // Realistic: the side being worked on, front unless it's the back.
        let front = view.layer != Layer::BCu;
        let outer = if front { Layer::FCu } else { Layer::BCu };
        // A slight lift separates the board edge from the surrounding workspace.
        let shadow = Draw { painter: &painter, cam, view: rect.translate(egui::vec2(0.0, 5.0)) };
        shadow.mesh(&analysis.face.0, &analysis.face.1, Color32::from_black_alpha(75));
        d.mesh(&analysis.face.0, &analysis.face.1, if real { MASK } else { BOARD_FILL });
        // Grid dots, when they're far enough apart to see.
        let step_px = view.grid as f32 * cam.scale;
        if step_px >= 8.0 {
            let lo = cam.to_world(rect, rect.left_top());
            let hi = cam.to_world(rect, rect.right_bottom());
            let g = view.grid;
            let dot = Color32::from_rgba_unmultiplied(139, 166, 185, 35);
            let mut y = (lo.y / g).floor() * g;
            while y <= hi.y {
                let mut x = (lo.x / g).floor() * g;
                while x <= hi.x {
                    let major =
                        ((x / g).round() as i64) % 5 == 0 && ((y / g).round() as i64) % 5 == 0;
                    painter.circle_filled(
                        d.at([x, y]),
                        if major { 1.1 } else { 0.65 },
                        if major {
                            Color32::from_rgba_unmultiplied(139, 166, 185, 65)
                        } else {
                            dot
                        },
                    );
                    x += g;
                }
                y += g;
            }
        }
        // A part being dragged: its pads and markings shown where it'll land.
        let dragged = match (&view.drag, &view.preview) {
            (Some((Drag::Footprint(path), _, _)), Some(preview)) => {
                layout.part(path).and_then(|k| {
                    let (old, new) = (board.placed(path)?, preview.placed(path)?);
                    Some((k, Place::of(old), Place::of(new)))
                })
            }
            _ => None,
        };
        let shift = |part: usize, q: P| match dragged {
            Some((k, old, new)) if k == part => new.apply(old.unapply(q)),
            _ => q,
        };
        // Tracks and vias as they are, or as the drag leaves them.
        let routed = view.preview.as_ref().unwrap_or(board);
        let net_hi = match &view.selected {
            Some(Pick::Track(k)) => routed.tracks.get(*k).map(|t| t.net.clone()),
            Some(Pick::Via(k)) => routed.vias.get(*k).map(|v| v.net.clone()),
            _ => None,
        }
        .or_else(|| view.highlight.clone());
        // A net's own colour, when net colours are on (F5).
        let net_colour = |net: &str, fallback: Color32| -> Color32 {
            match board.net_colours.get(net).filter(|_| view.net_colours) {
                Some([r, g, b]) => Color32::from_rgb(*r, *g, *b)
                    .gamma_multiply(if fallback.a() < 255 { 0.6 } else { 1.0 }),
                None => fallback,
            }
        };
        // Power analysis: the shown net's tracks by how hard they're worked.
        let power_load = match &view.power {
            Some(pw) if pw.revision == analysis.revision => pw.load(layout),
            _ => Default::default(),
        };
        // Bottom to top, the layer being worked on last.
        let mut layers = board.copper();
        layers.reverse();
        if let Some(k) = layers.iter().position(|l| *l == view.layer) {
            let active = layers.remove(k);
            layers.push(active);
        }
        let dim = if view.visible.high_contrast { 0.18 } else { 0.45 };
        for layer in layers {
            if real && layer != outer {
                continue;
            }
            if !real && (!view.visible.shows(layer) || (view.single_layer && layer != view.layer)) {
                continue;
            }
            let active = layer == view.layer;
            let base = layer_colour(layer);
            let colour = match (real, active) {
                (true, _) => MASK_COPPER,
                (false, true) => base,
                (false, false) => base.gamma_multiply(dim),
            };
            if view.visible.zones {
                for (l, pts, idx) in &analysis.fills {
                    if *l == layer {
                        d.mesh(pts, idx, colour.gamma_multiply(0.7));
                    }
                }
            }
            for t in analysis.teardrops.iter().filter(|t| t.layer == layer) {
                d.convex(&t.outline, net_colour(&t.net, colour));
            }
            for (k, t) in routed.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
                let hi = view.selected == Some(Pick::Track(k))
                    || net_hi.as_deref() == Some(t.net.as_str());
                let c = match power_load.get(&k) {
                    Some(load) if view.preview.is_none() => heat(*load),
                    _ => net_colour(&t.net, colour),
                };
                d.stroke(
                    &[p(t.a), p(t.b)],
                    t.width.mm(),
                    if hi { c.gamma_multiply(1.4).to_opaque() } else { c },
                );
            }
            for pad in layout.pads.iter().filter(|pd| pd.layers.contains(&layer)) {
                let mut c = geometry::pad(pad, 0.0);
                for q in &mut c {
                    *q = shift(pad.part, *q);
                }
                let hi = net_hi.is_some() && pad.net == net_hi;
                let colour = if real { GOLD } else { colour };
                d.convex(&c, if hi { colour.gamma_multiply(1.4).to_opaque() } else { colour });
            }
        }
        for (k, v) in routed.vias.iter().enumerate() {
            // Tented under the mask in the realistic view.
            d.disc(
                p(v.at),
                v.diameter.mm() / 2.0,
                match view.selected == Some(Pick::Via(k)) {
                    true => SELECTED,
                    false if real => MASK_COPPER,
                    false => VIA,
                },
            );
            d.disc(p(v.at), v.drill.mm() / 2.0, HOLE);
        }
        for pad in &layout.pads {
            if let Some(dr) = pad.drill() {
                d.disc(shift(pad.part, pad.center), dr / 2.0, HOLE);
            }
        }
        // Zone outlines.
        for (k, z) in board.zones.iter().enumerate() {
            if !view.visible.shows(z.layer) {
                continue;
            }
            let pts: Vec<P> = z.outline.iter().map(|q| p(*q)).collect();
            let c =
                if view.selected == Some(Pick::Zone(k)) { SELECTED } else { layer_colour(z.layer) };
            d.outline(&pts, Stroke::new(1.0, c));
        }
        // Keepouts: a red outline, hatched.
        for (k, z) in board.keepouts.iter().enumerate() {
            let pts: Vec<P> = z.outline.iter().map(|q| p(*q)).collect();
            if pts.len() < 3 {
                continue;
            }
            let c = if view.selected == Some(Pick::Keepout(k)) {
                SELECTED
            } else {
                Color32::from_rgb(230, 60, 60)
            };
            d.outline(&pts, Stroke::new(1.5, c));
            // Hatching: diagonal lines clipped to the outline, a millimetre apart.
            let shape = vec![vec![geometry::ccw(pts.clone())]];
            if let Some((lo, hi)) = geometry::bounds(&shape) {
                let span = (hi[0] - lo[0]) + (hi[1] - lo[1]);
                let mut t = 0.0;
                while t < span {
                    let a = [lo[0] + t, lo[1]];
                    let b = [lo[0] + t - (hi[1] - lo[1]), hi[1]];
                    let line = vec![vec![geometry::capsule(a, b, 0.05)]];
                    for piece in geometry::intersection(&line, &shape) {
                        if let Some(c0) = piece.first() {
                            if let Some((pl, ph)) = geometry::bounds(&vec![vec![c0.clone()]]) {
                                painter.line_segment(
                                    [d.at([ph[0], pl[1]]), d.at([pl[0], ph[1]])],
                                    Stroke::new(1.0, c.gamma_multiply(0.5)),
                                );
                            }
                        }
                    }
                    t += 1.0;
                }
            }
        }
        // Silkscreen, courtyards and fabrication drawings, as shown; realistic, only
        // the side's legend and the edge, in white.
        let realistic_visible;
        let shown = if real {
            let silk = if front { Layer::FSilk } else { Layer::BSilk };
            let mut v = view.visible.clone();
            v.hidden = OTHER_LAYERS
                .iter()
                .map(|(l, _)| *l)
                .filter(|l| *l != silk && *l != Layer::EdgeCuts)
                .collect();
            realistic_visible = v;
            &realistic_visible
        } else {
            &view.visible
        };
        for (k, part) in layout.parts.iter().enumerate() {
            if part.place.is_none() {
                continue;
            }
            for g in layout.graphics(k) {
                paint_graphic(&d, &g, &|q| shift(k, q), shown);
            }
        }
        for g in &board.graphics {
            paint_graphic(&d, g, &|q| q, shown);
        }
        // The edge.
        let edge: Vec<P> = board.edge().iter().map(|q| p(*q)).collect();
        if edge.len() >= 2 && view.visible.shows(Layer::EdgeCuts) {
            d.outline(&edge, Stroke::new(1.5, EDGE));
        }
        // Its handles, to shape it: squares on the corners, dots mid-edge to bend it.
        if view.tool == PcbTool::Select && view.visible.shows(Layer::EdgeCuts) {
            for (k, q) in board.outline.iter().enumerate() {
                let on = view.selected == Some(Pick::Corner(k));
                let r = egui::Rect::from_center_size(d.at(p(*q)), egui::vec2(7.0, 7.0));
                painter.rect_filled(r, 1.0, if on { SELECTED } else { BACKGROUND });
                painter.rect_stroke(r, 1.0, Stroke::new(1.2, EDGE), egui::StrokeKind::Middle);
                painter.circle_stroke(d.at(edge_middle(board, k)), 3.0, Stroke::new(1.2, EDGE));
            }
        }
        if let Some(Pick::Cutout(k)) = view.selected {
            if let Some(c) = board.cutouts().into_iter().nth(
                board.graphics[..k.min(board.graphics.len())]
                    .iter()
                    .filter(|g| cutout_graphic(g))
                    .count(),
            ) {
                let c: Vec<P> = c.iter().map(|q| p(*q)).collect();
                d.outline(&c, Stroke::new(2.0, SELECTED));
            }
        }
        // The selected part's box.
        if let Some(Pick::Footprint(path)) = &view.selected {
            if let Some(k) = layout.part(path) {
                let part = &layout.parts[k];
                if let Some(place) = part.place {
                    let (lo, hi) = sync::extent(&part.footprint);
                    let c: Vec<P> = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
                        .iter()
                        .map(|&q| shift(k, place.apply(q)))
                        .collect();
                    let locked = board.placed(path).is_some_and(|f| f.locked);
                    d.outline(
                        &c,
                        Stroke::new(
                            1.5,
                            if locked { Color32::from_rgb(255, 170, 60) } else { SELECTED },
                        ),
                    );
                }
            }
        }
        // The group, and the selection box.
        for path in &view.group {
            if let Some(k) = layout.part(path) {
                if let Some((lo, hi)) = circuiter_pcb::edit::part_box(layout, k) {
                    let c: Vec<P> = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
                        .iter()
                        .map(|&q| shift(k, q))
                        .collect();
                    d.outline(&c, Stroke::new(1.5, Color32::from_rgb(120, 200, 255)));
                }
            }
        }
        if let Some((a, b)) = view.band {
            let c = [a, [b[0], a[1]], b, [a[0], b[1]]];
            d.outline(&c, Stroke::new(1.0, Color32::from_rgb(120, 200, 255)));
        }
        if let Some(checked) = &analysis.checked {
            if view.visible.ratsnest {
                for (_, a, b) in &checked.connectivity.ratsnest {
                    painter.line_segment([d.at(*a), d.at(*b)], Stroke::new(1.0, RATSNEST));
                }
            }
            if view.visible.drc {
                for v in &checked.violations {
                    let Some(at) = v.at else { continue };
                    if v.rule == drc::Rule::Unconnected {
                        continue;
                    }
                    let c = if v.severity() == Severity::Error {
                        Color32::from_rgb(255, 60, 60)
                    } else {
                        Color32::from_rgb(255, 190, 40)
                    };
                    let s = d.at(at);
                    painter.add(egui::Shape::convex_polygon(
                        vec![s, s + egui::vec2(10.0, -4.0), s + egui::vec2(4.0, -10.0)],
                        c,
                        Stroke::NONE,
                    ));
                }
            }
        }
        // What's being drawn.
        let ghost = Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 255, 255, 180));
        match &view.tool {
            PcbTool::Route { net, width, points } if !points.is_empty() => {
                if let Some((path, blocked)) = &view.route_preview {
                    let colour = if *blocked {
                        Color32::from_rgb(255, 80, 80)
                    } else {
                        layer_colour(view.layer).gamma_multiply(1.3).to_opaque()
                    };
                    d.stroke(path, *width, colour.gamma_multiply(0.8));
                    // The clearance it keeps, faintly.
                    let clearance = layout.board.rules.clearance_for(net).mm();
                    for seg in path.windows(2) {
                        d.outline(
                            &geometry::capsule(seg[0], seg[1], width + 2.0 * clearance),
                            Stroke::new(1.0, colour.gamma_multiply(0.35)),
                        );
                    }
                    let _ = points;
                }
            }
            PcbTool::Measure { from: Some(f), to } => {
                if let Some(t) = to.or(view.cursor.map(|c| view.snap(c))) {
                    let (a, b) = (d.at(*f), d.at(t));
                    let ruler = Stroke::new(1.5, Color32::from_rgb(255, 230, 120));
                    painter.line_segment([a, b], ruler);
                    for e in [a, b] {
                        painter.circle_stroke(e, 4.0, ruler);
                    }
                    let (dx, dy) = (t[0] - f[0], t[1] - f[1]);
                    painter.text(
                        b + egui::vec2(10.0, -10.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{:.3} mm\nΔX {dx:.3}  ΔY {dy:.3}", dx.hypot(dy)),
                        egui::FontId::monospace(12.0),
                        Color32::from_rgb(255, 230, 120),
                    );
                }
            }
            PcbTool::Zone { points }
            | PcbTool::Outline { points }
            | PcbTool::Keepout { points }
            | PcbTool::Cutout { points }
                if !points.is_empty() =>
            {
                let mut pts = points.clone();
                if let Some(c) = view.cursor {
                    pts.push(view.snap(c));
                }
                let screen: Vec<Pos2> = pts.iter().map(|&q| d.at(q)).collect();
                painter.add(egui::Shape::line(screen, ghost));
            }
            _ => {}
        }
        if let Some(c) = view.cursor {
            if view.tool != PcbTool::Select {
                let s = d.at(view.snap(c));
                painter.line_segment([s - egui::vec2(6.0, 0.0), s + egui::vec2(6.0, 0.0)], ghost);
                painter.line_segment([s - egui::vec2(0.0, 6.0), s + egui::vec2(0.0, 6.0)], ghost);
            }
        }
        // Status, faintly, in the corner.
        let mut status = String::new();
        if let Some(c) = view.cursor {
            status = format!("X {:.3}  Y {:.3} mm   ", c[0], c[1]);
        }
        if let Some(checked) = &analysis.checked {
            let un = checked.connectivity.unrouted();
            status.push_str(&format!(
                "{} unrouted · {} error(s) · {} warning(s)",
                un,
                checked.errors(),
                checked.warnings()
            ));
        }
        if let Some(n) = &view.note {
            status.push_str(&format!("   {n}"));
        }
        // PCB rendering uses a dark work surface in either application theme.
        let pal = &style::DARK;
        if rect.width() > 340.0 && rect.height() > 160.0 {
            style::canvas_badge(
                &painter,
                rect.left_top() + egui::vec2(14.0, 14.0),
                egui::Align2::LEFT_TOP,
                &format!(
                    "{}  ·  {}  ·  {} mm grid",
                    view.tool.name(),
                    view.layer.name(),
                    view.grid
                ),
                pal,
                layer_colour(view.layer),
            );
            // Leave room for the scale at the right and clip long cursor / route notes.
            let status_painter = painter.with_clip_rect(egui::Rect::from_min_max(
                rect.min,
                rect.right_bottom() - egui::vec2(138.0, 0.0),
            ));
            let health = if analysis.checked.as_ref().is_some_and(|c| c.errors() > 0) {
                pal.error
            } else if analysis
                .checked
                .as_ref()
                .is_some_and(|c| c.warnings() > 0 || c.connectivity.unrouted() > 0)
            {
                pal.warning
            } else {
                pal.go
            };
            style::canvas_badge(
                &status_painter,
                rect.left_bottom() + egui::vec2(14.0, -14.0),
                egui::Align2::LEFT_BOTTOM,
                &status,
                pal,
                health,
            );
        }
        style::canvas_scale(&painter, rect, cam.scale, pal);
    }
}

/// A drawing on a silkscreen, courtyard or fabrication layer, if that layer shows.
fn paint_graphic(
    d: &Draw<'_>,
    g: &circuiter_model::board::Graphic,
    shift: &dyn Fn(P) -> P,
    visible: &Visible,
) {
    use circuiter_model::board::Graphic as G;
    let colour = |l: Layer| {
        let drawn = matches!(
            l,
            Layer::FSilk
                | Layer::BSilk
                | Layer::FCrtYd
                | Layer::BCrtYd
                | Layer::FFab
                | Layer::BFab
                | Layer::EdgeCuts
        );
        let silk = matches!(l, Layer::FSilk | Layer::BSilk);
        (drawn && visible.shows(l))
            .then(|| if visible.realistic && silk { LEGEND } else { layer_colour(l) })
    };
    match g {
        G::Line { layer, a, b, width } => {
            if let Some(c) = colour(*layer) {
                d.stroke(&[shift(p(*a)), shift(p(*b))], width.mm(), c);
            }
        }
        G::Circle { layer, center, radius, width } => {
            if let Some(c) = colour(*layer) {
                d.painter.circle_stroke(
                    d.at(shift(p(*center))),
                    radius.mm() as f32 * d.cam.scale,
                    Stroke::new(d.px(width.mm()), c),
                );
            }
        }
        G::Poly { layer, points, width, filled } => {
            if let Some(c) = colour(*layer) {
                let pts: Vec<P> = points.iter().map(|q| shift(p(*q))).collect();
                if *filled {
                    d.convex(&pts, c);
                } else {
                    let mut closed = pts.clone();
                    closed.extend(pts.first().copied());
                    d.stroke(&closed, width.mm(), c);
                }
            }
        }
        G::Text { layer, at, text, size } => {
            if let Some(c) = colour(*layer) {
                for line in
                    font::strokes(text, shift(p(*at)), size.mm(), 0.0, *layer == Layer::BSilk)
                {
                    d.stroke(&line, font::stroke(size.mm()), c);
                }
            }
        }
    }
}

/// The board's settings and the selected item, for the inspector.
pub fn board_inspector(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let pal = style::palette(ui);
    let Some(ws) = &app.workspace else { return };
    let editable = ws.editable();
    let Some(board) = ws.current_def().and_then(|d| d.board.clone()) else {
        ui.label(
            RichText::new("No board yet: create one from the PCB view's button.").color(pal.weak),
        );
        return;
    };
    let mut changed = board.clone();
    let selected = app.pcb.selected.clone();
    // 3D parts of the project, for a case of your own.
    let bodies: Vec<String> = ws
        .design
        .library(ws.project.name())
        .map(|l| {
            l.cells
                .values()
                .filter(|d| d.model3d.is_some() && d.schematic.is_none())
                .map(|d| d.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut make_parts = false;
    let mut import_error = None;
    let layout = app.pcb.layout().cloned();
    // The panel as set, made up: its size, or why it can't be.
    let panel = match (&board.panel, &app.pcb.analysis) {
        (Some(pn), Some(a)) => match (&a.layout, &a.checked, &app.pcb.panel_made) {
            (_, _, Some((rev, made))) if *rev == a.revision => Some(made.clone()),
            (Ok(l), Some(c), _) => {
                let made = circuiter_pcb::panel::panelize(l, &c.fills, pn)
                    .map(|m| (m.size, m.copies.len()));
                app.pcb.panel_made = Some((a.revision, made.clone()));
                Some(made)
            }
            _ => None,
        },
        _ => None,
    };
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            if let Some(pick) = &selected {
                style::section(ui, "Selected", |ui| {
                    selected_ui(ui, &mut changed, layout.as_ref(), pick)
                });
            }
            style::section(ui, "Board", |ui| {
                let outline: Vec<P> = changed.outline.iter().map(|q| p(*q)).collect();
                let rect = outline.len() == 4 && changed.bulges.is_empty() && {
                    let (xs, ys): (Vec<f64>, Vec<f64>) =
                        outline.iter().map(|q| (q[0], q[1])).unzip();
                    xs.iter().all(|&x| {
                        (x - xs[0]).abs() < 1e-9
                            || (x - xs[1]).abs() < 1e-9
                            || (x - xs[2]).abs() < 1e-9
                    }) && ys.iter().filter(|&&y| (y - ys[0]).abs() < 1e-9).count() == 2
                };
                style::properties(ui, "board-props", |ui| {
                    if rect {
                        let (lo, hi) =
                            outline.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
                                (
                                    [lo[0].min(q[0]), lo[1].min(q[1])],
                                    [hi[0].max(q[0]), hi[1].max(q[1])],
                                )
                            });
                        let (mut w, mut h) = (hi[0] - lo[0], hi[1] - lo[1]);
                        style::name(ui, "Width");
                        let a = ui
                            .add(
                                egui::DragValue::new(&mut w)
                                    .range(5.0..=1000.0)
                                    .speed(0.5)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                        style::name(ui, "Height");
                        let b = ui
                            .add(
                                egui::DragValue::new(&mut h)
                                    .range(5.0..=1000.0)
                                    .speed(0.5)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                        if a || b {
                            let pt = |x: f64, y: f64| point([x, y]);
                            changed.outline = vec![
                                pt(lo[0], lo[1]),
                                pt(lo[0] + w, lo[1]),
                                pt(lo[0] + w, lo[1] + h),
                                pt(lo[0], lo[1] + h),
                            ];
                        }
                    } else {
                        let edge: Vec<P> = changed.edge().iter().map(|q| p(*q)).collect();
                        let (lo, hi) =
                            geometry::bounds(&vec![vec![edge]]).unwrap_or(([0.0; 2], [0.0; 2]));
                        style::name(ui, "Outline");
                        let arcs = changed.bulges.iter().filter(|b| **b != 0.0).count();
                        ui.label(format!(
                            "{} corners, {arcs} arcs · {:.2} × {:.2} mm",
                            outline.len(),
                            hi[0] - lo[0],
                            hi[1] - lo[1]
                        ));
                        ui.end_row();
                    }
                    style::name(ui, "Corners");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut app.pcb.corner_radius)
                                .range(0.1..=200.0)
                                .speed(0.1)
                                .suffix(" mm"),
                        );
                        if ui
                            .button("Round")
                            .on_hover_text(
                                "Round every corner between straight edges to this radius",
                            )
                            .clicked()
                        {
                            changed.round_corners(app.pcb.corner_radius);
                        }
                        if ui
                            .button("Straighten")
                            .on_hover_text("Make every arc of the outline a straight edge")
                            .clicked()
                        {
                            changed.bulges.clear();
                        }
                    });
                    ui.end_row();
                    style::name(ui, "");
                    if ui
                        .button(format!("{}  Import Outline…", icon::POLYGON))
                        .on_hover_text(
                            "The board's edge from a DXF or SVG drawing: its largest outline",
                        )
                        .clicked()
                    {
                        match import_outline() {
                            Ok(Some(points)) => changed.outline = points,
                            Ok(None) => {}
                            Err(e) => import_error = Some(e),
                        }
                    }
                    ui.end_row();
                    style::name(ui, "Copper layers");
                    egui::ComboBox::from_id_salt("copper-layers")
                        .selected_text(changed.copper_layers.to_string())
                        .show_ui(ui, |ui| {
                            for n in [2u8, 4, 6] {
                                ui.selectable_value(&mut changed.copper_layers, n, n.to_string());
                            }
                        });
                    ui.end_row();
                    length_row(ui, "Thickness", &mut changed.thickness, 0.4..=3.2);
                });
            });
            style::section(ui, "Design rules", |ui| {
                style::properties(ui, "rules-props", |ui| {
                    let r = &mut changed.rules;
                    length_row(ui, "Clearance", &mut r.clearance, 0.05..=5.0);
                    length_row(ui, "Track width", &mut r.track_width, 0.05..=10.0);
                    length_row(ui, "Via diameter", &mut r.via_diameter, 0.2..=5.0);
                    length_row(ui, "Via drill", &mut r.via_drill, 0.1..=4.0);
                    length_row(ui, "Min. track", &mut r.min_track_width, 0.05..=5.0);
                    length_row(ui, "Min. clearance", &mut r.min_clearance, 0.05..=5.0);
                    length_row(ui, "Min. drill", &mut r.min_drill, 0.1..=5.0);
                    length_row(ui, "Min. ring", &mut r.min_annular_ring, 0.05..=2.0);
                    length_row(ui, "To edge", &mut r.edge_clearance, 0.0..=5.0);
                    length_row(ui, "Hole to hole", &mut r.hole_to_hole, 0.0..=5.0);
                });
                ui.label(
                    RichText::new("The defaults suit most board houses' standard service.")
                        .color(pal.weak)
                        .small(),
                );
            });
            style::section(ui, "Case", |ui| {
                case_ui(ui, &mut changed, &bodies, &mut make_parts, pal)
            });
            style::section(ui, "Panel", |ui| panel_ui(ui, &mut changed, panel.as_ref(), pal));
            style::section(ui, "Variants", |ui| variants_ui(ui, &mut changed, pal));
        });
    });
    if make_parts {
        app.pcb_case_parts();
        return;
    }
    if let Some(e) = import_error {
        app.error = Some(e);
    }
    if changed != board {
        let Some(ws) = &mut app.workspace else { return };
        if let Err(e) = ws.edit_board(|_, _, b| {
            *b = Some(changed);
            Ok(())
        }) {
            app.error = Some(e);
        }
    }
}

/// A board outline from a drawing the user picks: its largest outline, arcs drawn as
/// chords, moved so its top-left corner is the board's origin. `None` if they cancel.
fn import_outline() -> Result<Option<Vec<circuiter_model::geom::Point>>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Drawings", &["dxf", "svg"]).pick_file()
    else {
        return Ok(None);
    };
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let loops = circuiter_3d::import2d::outlines(&path.to_string_lossy(), &text)?;
    Ok(Some(board_outline(&loops[0])))
}

/// A drawing's outline (Y up) as a board's (Y down), its top-left corner at the origin.
pub fn board_outline(l: &circuiter_3d::import2d::Loop) -> Vec<circuiter_model::geom::Point> {
    let pts = circuiter_3d::import2d::flatten(l);
    let x0 = pts.iter().map(|q| q.0).fold(f64::MAX, f64::min);
    let y1 = pts.iter().map(|q| q.1).fold(f64::MIN, f64::max);
    pts.iter().map(|&(x, y)| point([x - x0, y1 - y])).collect()
}

/// Assembly variants: which is being built, and the list of them. What each changes
/// is set on the parts themselves (select one).
fn variants_ui(ui: &mut egui::Ui, board: &mut Board, pal: &style::Palette) {
    use circuiter_model::board::Variant;
    ui.label(RichText::new("The same board built differently: parts left off, other values. Pick the variant to work on, then set how each part differs in its properties. The BOM, placement file, 3D and drawings follow it.").color(pal.weak));
    style::properties(ui, "variant-props", |ui| {
        style::name(ui, "Building");
        let current = board.active_variant().map_or("As drawn".to_owned(), |v| v.name.clone());
        egui::ComboBox::from_id_salt("variant-active").selected_text(current).show_ui(ui, |ui| {
            if ui.selectable_label(board.variant.is_none(), "As drawn").clicked() {
                board.variant = None;
            }
            for v in &board.variants {
                if ui.selectable_label(board.variant.as_ref() == Some(&v.name), &v.name).clicked() {
                    board.variant = Some(v.name.clone());
                }
            }
        });
        ui.end_row();
    });
    let mut remove = None;
    for (k, v) in board.variants.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let old = v.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(110.0)).changed()
                && board.variant.as_ref() == Some(&old)
            {
                board.variant = Some(v.name.clone());
            }
            let off = v.parts.values().filter(|o| o.not_fitted).count();
            let changed =
                v.parts.values().filter(|o| o.value.is_some() || !o.fields.is_empty()).count();
            ui.label(
                RichText::new(format!("{off} off · {changed} changed")).color(pal.weak).small(),
            );
            if ui.small_button(icon::TRASH).on_hover_text("Delete the variant").clicked() {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = remove {
        let gone = board.variants.remove(k);
        if board.variant.as_ref() == Some(&gone.name) {
            board.variant = None;
        }
    }
    if ui.button(format!("{}  New Variant", icon::PLUS)).clicked() {
        let name = (1..)
            .map(|k| format!("Variant {k}"))
            .find(|n| !board.variants.iter().any(|v| v.name == *n))
            .expect("a name");
        board.variants.push(Variant { name: name.clone(), ..Variant::default() });
        board.variant = Some(name);
    }
}

/// The panel copies of the board are made in: rows and columns, rails, how they
/// separate.
fn panel_ui(ui: &mut egui::Ui, board: &mut Board, made: Option<&PanelMade>, pal: &style::Palette) {
    use circuiter_model::board::Separation;
    let Some(pn) = &mut board.panel else {
        ui.label(RichText::new("Copies of the board made, assembled and tested together, then snapped apart: rails for the assembly line, fiducials, tooling holes, and V-scores or mouse bites.").color(pal.weak));
        if ui.button(format!("{}  Make a Panel", icon::GRID_FOUR)).clicked() {
            board.panel = Some(Default::default());
        }
        return;
    };
    let mut remove = false;
    style::properties(ui, "panel-props", |ui| {
        style::name(ui, "Copies");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut pn.columns).range(1..=20).suffix(" across"));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut pn.rows).range(1..=20).suffix(" down"));
        });
        ui.end_row();
        length_row(ui, "Rails", &mut pn.rails, 0.0..=20.0);
        style::name(ui, "Separate by");
        let bites = matches!(pn.separation, Separation::MouseBites { .. });
        ui.horizontal(|ui| {
            if ui.selectable_label(!bites, "V-score").clicked() {
                pn.separation = Separation::VScore;
            }
            if ui.selectable_label(bites, "Mouse bites").clicked() && !bites {
                pn.separation = Separation::default();
            }
        });
        ui.end_row();
        if let Separation::MouseBites { gap, tab } = &mut pn.separation {
            length_row(ui, "Routed gap", gap, 0.5..=10.0);
            length_row(ui, "Tab width", tab, 1.0..=20.0);
        }
        style::name(ui, "On the rails");
        ui.horizontal(|ui| {
            ui.checkbox(&mut pn.fiducials, "Fiducials");
            ui.checkbox(&mut pn.tooling, "Tooling holes");
        });
        ui.end_row();
    });
    match made {
        Some(Ok(((w, h), n))) => {
            ui.label(RichText::new(format!("{n} copies on a {w:.1} × {h:.1} mm panel; Fabrication Files writes its files too.")).color(pal.weak).small());
        }
        Some(Err(e)) => {
            ui.colored_label(pal.error, e);
        }
        None => {}
    }
    if ui.button(format!("{}  No Panel", icon::TRASH)).clicked() {
        remove = true;
    }
    if remove {
        board.panel = None;
    }
}

/// The board's case: made to fit, or a part of your own; its size, material and use.
fn case_ui(
    ui: &mut egui::Ui,
    board: &mut Board,
    bodies: &[String],
    make_parts: &mut bool,
    pal: &style::Palette,
) {
    let Some(e) = &mut board.enclosure else {
        ui.label(RichText::new("A base and lid made to fit the board, with standoffs and openings for its connectors and controls; or a 3D part of your own.").color(pal.weak));
        if ui.button(format!("{}  Make a Case", icon::CUBE)).clicked() {
            board.enclosure = Some(Default::default());
        }
        return;
    };
    let mut remove = false;
    style::properties(ui, "case-props", |ui| {
        style::name(ui, "Case");
        let current = e.part.as_ref().map_or("Made to fit".to_owned(), |p| p.to_string());
        egui::ComboBox::from_id_salt("case-part").selected_text(current).show_ui(ui, |ui| {
            if ui.selectable_label(e.part.is_none(), "Made to fit").clicked() {
                e.part = None;
                e.lid = None;
            }
            for name in bodies {
                let r = circuiter_model::CellRef::new(None, name);
                if ui.selectable_label(e.part.as_ref() == Some(&r), name).clicked() {
                    e.part = Some(r);
                }
            }
        });
        ui.end_row();
        if e.part.is_some() {
            style::name(ui, "Lid");
            let current = e.lid.as_ref().map_or("None".to_owned(), |p| p.to_string());
            egui::ComboBox::from_id_salt("case-lid").selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(e.lid.is_none(), "None").clicked() {
                    e.lid = None;
                }
                for name in bodies {
                    let r = circuiter_model::CellRef::new(None, name);
                    if ui.selectable_label(e.lid.as_ref() == Some(&r), name).clicked() {
                        e.lid = Some(r);
                    }
                }
            });
            ui.end_row();
            style::name(ui, "Board at");
            ui.horizontal(|ui| {
                for v in &mut e.board_at {
                    ui.add(egui::DragValue::new(v).speed(0.1).suffix(" mm"));
                }
            });
            ui.end_row();
        }
        let mm = |ui: &mut egui::Ui, name: &str, v: &mut f64, lo: f64, hi: f64, tip: &str| {
            style::name(ui, name);
            ui.add(egui::DragValue::new(v).range(lo..=hi).speed(0.1).suffix(" mm"))
                .on_hover_text(tip);
            ui.end_row();
        };
        mm(ui, "Wall", &mut e.wall, 0.5, 10.0, "Wall thickness");
        if e.part.is_none() {
            mm(ui, "Gap", &mut e.gap, 0.2, 20.0, "Room between the board and the walls");
            mm(
                ui,
                "Under the board",
                &mut e.below,
                0.0,
                100.0,
                "Standoff height; 0 clears the parts' leads",
            );
            mm(
                ui,
                "Over the board",
                &mut e.above,
                0.0,
                200.0,
                "Headroom; 0 clears the tallest part",
            );
            mm(ui, "Corner radius", &mut e.radius, 0.0, 30.0, "Rounding of the outside corners");
        }
        style::name(ui, "Material");
        let mut m = usize::from(e.material == circuiter_model::board::CaseMaterial::Aluminium);
        if let Some(k) = style::segmented(ui, &[("Plastic", true), ("Aluminium", true)], m) {
            m = k;
            e.material = if m == 0 {
                circuiter_model::board::CaseMaterial::Plastic
            } else {
                circuiter_model::board::CaseMaterial::Aluminium
            };
        }
        ui.end_row();
        style::name(ui, "Vents");
        ui.checkbox(&mut e.vents, "Slots in the lid");
        ui.end_row();
        if e.part.is_none() {
            style::name(ui, "Openings");
            ui.checkbox(&mut e.openings, "For connectors and controls");
            ui.end_row();
        }
        style::name(ui, "Simulate in it");
        ui.checkbox(&mut e.simulate, "The air inside warms with the parts");
        ui.end_row();
        style::name(ui, "Room");
        ui.add(egui::DragValue::new(&mut e.room).range(-40.0..=85.0).speed(0.5).suffix(" °C"));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        if e.part.is_none()
            && ui
                .button(format!("{}  Make Editable 3D Parts", icon::CUBE))
                .on_hover_text(
                    "Turns the case into two 3D parts, base and lid, to change in the body editor",
                )
                .clicked()
        {
            *make_parts = true;
        }
        if ui.button(format!("{}  Remove Case", icon::TRASH)).clicked() {
            remove = true;
        }
    });
    if remove {
        board.enclosure = None;
    }
}

fn length_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut Length,
    range: std::ops::RangeInclusive<f64>,
) {
    style::name(ui, name);
    let mut mm = value.mm();
    if ui
        .add(egui::DragValue::new(&mut mm).range(range).speed(0.01).max_decimals(3).suffix(" mm"))
        .changed()
    {
        *value = Length::from_mm(mm);
    }
    ui.end_row();
}

fn selected_ui(ui: &mut egui::Ui, board: &mut Board, layout: Option<&Layout>, pick: &Pick) {
    let p = style::palette(ui);
    match pick {
        Pick::Footprint(path) => {
            let part = layout.and_then(|l| l.part(path).map(|k| &l.parts[k]));
            let Some(fp) = board.footprints.iter_mut().find(|f| f.path == *path) else { return };
            if let Some(part) = part {
                ui.label(
                    RichText::new(format!(
                        "{}  {}  ·  {}",
                        part.reference, part.value, part.package
                    ))
                    .strong()
                    .color(p.text),
                );
            }
            style::properties(ui, "fp-props", |ui| {
                let mut at = p_of(fp);
                style::name(ui, "X");
                let a = ui.add(egui::DragValue::new(&mut at[0]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                style::name(ui, "Y");
                let b = ui.add(egui::DragValue::new(&mut at[1]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                if a || b {
                    fp.at = point(at);
                }
                style::name(ui, "Rotation");
                ui.add(
                    egui::DragValue::new(&mut fp.rotation)
                        .range(0.0..=359.9)
                        .speed(1.0)
                        .suffix("°"),
                );
                ui.end_row();
                style::name(ui, "Side");
                let mut side = usize::from(fp.back);
                if let Some(s) = style::segmented(ui, &[("Front", true), ("Back", true)], side) {
                    side = s;
                    fp.back = side == 1;
                }
                ui.end_row();
                style::name(ui, "Locked");
                ui.checkbox(&mut fp.locked, "");
                ui.end_row();
            });
            // How it differs in the variant being built.
            // (Shown as the hint only while the variant leaves it as designed.)
            let design_value = part.map(|pt| pt.value.clone()).unwrap_or_default();
            let active = board.variant.clone();
            if let Some(v) = board.variants.iter_mut().find(|v| Some(&v.name) == active.as_ref()) {
                ui.label(RichText::new(format!("In {}", v.name)).strong().color(p.text));
                let mut o = v.parts.get(path).cloned().unwrap_or_default();
                style::properties(ui, "fp-variant", |ui| {
                    style::name(ui, "Assembly");
                    let mut fitted = !o.not_fitted;
                    if ui.checkbox(&mut fitted, "Fitted").changed() {
                        o.not_fitted = !fitted;
                    }
                    ui.end_row();
                    style::name(ui, "Value");
                    let mut text = o.value.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut text)
                                .hint_text(design_value)
                                .desired_width(120.0),
                        )
                        .changed()
                    {
                        o.value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
                    }
                    ui.end_row();
                });
                if o == Default::default() {
                    v.parts.remove(path);
                } else {
                    v.parts.insert(path.clone(), o);
                }
            }
        }
        Pick::Track(k) => {
            let Some(t) = board.tracks.get_mut(*k) else { return };
            ui.label(RichText::new(format!("Track · {}", t.net)).strong().color(p.text));
            style::properties(ui, "track-props", |ui| {
                length_row(ui, "Width", &mut t.width, 0.05..=10.0);
                style::name(ui, "Layer");
                let mut side = usize::from(t.layer == Layer::BCu);
                if let Some(s) = style::segmented(ui, &[("F.Cu", true), ("B.Cu", true)], side) {
                    side = s;
                    t.layer = if side == 0 { Layer::FCu } else { Layer::BCu };
                }
                ui.end_row();
            });
            let len = (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm());
            ui.label(RichText::new(format!("{len:.2} mm long")).color(p.weak));
        }
        Pick::Via(k) => {
            let Some(v) = board.vias.get_mut(*k) else { return };
            ui.label(RichText::new(format!("Via · {}", v.net)).strong().color(p.text));
            style::properties(ui, "via-props", |ui| {
                length_row(ui, "Diameter", &mut v.diameter, 0.2..=5.0);
                length_row(ui, "Drill", &mut v.drill, 0.1..=4.0);
            });
        }
        Pick::Corner(k) => {
            let n = board.outline.len();
            if *k >= n {
                return;
            }
            ui.label(RichText::new(format!("Board corner {}", k + 1)).strong().color(p.text));
            style::properties(ui, "corner-props", |ui| {
                let mut at = circuiter_pcb::layout::p(board.outline[*k]);
                style::name(ui, "X");
                let x = ui.add(egui::DragValue::new(&mut at[0]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                style::name(ui, "Y");
                let y = ui.add(egui::DragValue::new(&mut at[1]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                if x || y {
                    board.outline[*k] = point(at);
                }
                // The edge on to the next corner: straight, or an arc of a radius.
                board.bulges.resize(n, 0.0);
                let (a, b) = (
                    circuiter_pcb::layout::p(board.outline[*k]),
                    circuiter_pcb::layout::p(board.outline[(*k + 1) % n]),
                );
                let half = (b[0] - a[0]).hypot(b[1] - a[1]) / 2.0;
                let bulge = board.bulges[*k];
                let mut radius = if bulge == 0.0 {
                    0.0
                } else {
                    let s = bulge * half;
                    (half * half + s * s) / (2.0 * s)
                };
                style::name(ui, "Next edge");
                let r = ui
                    .add(egui::DragValue::new(&mut radius).speed(0.1).suffix(" mm"))
                    .on_hover_text(
                        "The radius of the edge on to the next corner: 0 is straight, negative bends it inwards",
                    )
                    .changed();
                ui.end_row();
                if r {
                    // The bulge of the shorter arc of that radius through both corners.
                    board.bulges[*k] = if radius.abs() < half {
                        bulge
                    } else {
                        let s = radius - radius.signum() * (radius * radius - half * half).sqrt();
                        s / half
                    };
                }
                board.tidy_bulges();
            });
            ui.label(RichText::new("Drag it on the board · Del removes it").color(p.weak));
        }
        Pick::Cutout(k) => {
            ui.label(RichText::new("Cutout").strong().color(p.text));
            match board.graphics.get_mut(*k) {
                Some(Graphic::Circle { center, radius, .. }) => {
                    style::properties(ui, "cutout-props", |ui| {
                        let mut c = circuiter_pcb::layout::p(*center);
                        style::name(ui, "X");
                        ui.add(egui::DragValue::new(&mut c[0]).speed(0.1).suffix(" mm"));
                        ui.end_row();
                        style::name(ui, "Y");
                        ui.add(egui::DragValue::new(&mut c[1]).speed(0.1).suffix(" mm"));
                        ui.end_row();
                        *center = point(c);
                        let mut d = radius.mm() * 2.0;
                        style::name(ui, "Diameter");
                        if ui
                            .add(
                                egui::DragValue::new(&mut d)
                                    .range(0.3..=200.0)
                                    .speed(0.1)
                                    .suffix(" mm"),
                            )
                            .changed()
                        {
                            *radius = Length::from_mm(d / 2.0);
                        }
                        ui.end_row();
                    });
                }
                Some(Graphic::Poly { points, .. }) => {
                    ui.label(format!("{} corners", points.len()));
                }
                _ => {}
            }
            ui.label(RichText::new("Drag it on the board · Del removes it").color(p.weak));
        }
        Pick::Keepout(k) => {
            let copper = board.copper();
            let Some(z) = board.keepouts.get_mut(*k) else { return };
            ui.label(RichText::new("Keepout").strong().color(p.text));
            ui.label(RichText::new("Kept clear of:").color(p.weak));
            ui.checkbox(&mut z.tracks, "Tracks");
            ui.checkbox(&mut z.vias, "Vias");
            ui.checkbox(&mut z.pours, "Copper pours");
            ui.checkbox(&mut z.parts, "Parts");
            ui.label(RichText::new("On:").color(p.weak));
            let mut all = z.layers.is_empty();
            if ui.checkbox(&mut all, "Every copper layer").changed() {
                z.layers = if all { Vec::new() } else { vec![Layer::FCu] };
            }
            if !all {
                for l in copper {
                    let mut on = z.layers.contains(&l);
                    if ui.checkbox(&mut on, l.name()).changed() {
                        if on {
                            z.layers.push(l);
                        } else {
                            z.layers.retain(|x| *x != l);
                        }
                    }
                }
            }
        }
        Pick::Zone(k) => {
            let nets: Vec<String> =
                layout.map(|l| l.nets.iter().cloned().collect()).unwrap_or_default();
            let Some(z) = board.zones.get_mut(*k) else { return };
            ui.label(
                RichText::new(format!("Copper pour on {}", z.layer.name())).strong().color(p.text),
            );
            style::properties(ui, "zone-props", |ui| {
                style::name(ui, "Net");
                egui::ComboBox::from_id_salt("zone-net-sel").selected_text(z.net.clone()).show_ui(
                    ui,
                    |ui| {
                        for n in &nets {
                            ui.selectable_value(&mut z.net, n.clone(), n);
                        }
                    },
                );
                ui.end_row();
                style::name(ui, "Layer");
                let mut side = usize::from(z.layer == Layer::BCu);
                if let Some(s) = style::segmented(ui, &[("F.Cu", true), ("B.Cu", true)], side) {
                    side = s;
                    z.layer = if side == 0 { Layer::FCu } else { Layer::BCu };
                }
                ui.end_row();
                style::name(ui, "Priority");
                ui.add(egui::DragValue::new(&mut z.priority).range(0..=99));
                ui.end_row();
                style::name(ui, "Pads");
                egui::ComboBox::from_id_salt("zone-pads")
                    .selected_text(format!("{:?}", z.pads))
                    .show_ui(ui, |ui| {
                        for c in [PadConnection::Thermal, PadConnection::Solid, PadConnection::None]
                        {
                            ui.selectable_value(&mut z.pads, c, format!("{c:?}"));
                        }
                    });
                ui.end_row();
                length_row(ui, "Min. width", &mut z.min_width, 0.05..=5.0);
                length_row(ui, "Relief gap", &mut z.thermal_gap, 0.1..=5.0);
                length_row(ui, "Spoke width", &mut z.spoke_width, 0.1..=5.0);
            });
        }
    }
}

fn p_of(f: &Placed) -> P {
    p(f.at)
}

/// The design rule check's findings, for the results panel.
pub fn drc_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let p = style::palette(ui);
    let Some(checked) = app.pcb.analysis.as_ref().and_then(|a| a.checked.as_ref()) else {
        ui.label(RichText::new("No board to check.").color(p.weak));
        return;
    };
    let mut focus = None;
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if checked.violations.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{}  Every design rule is met and every net is routed.",
                    icon::CHECK_CIRCLE
                ))
                .color(p.go),
            );
        }
        for v in &checked.violations {
            let (glyph, c) = match v.severity() {
                Severity::Error => (icon::X_CIRCLE, p.error),
                Severity::Warning => (icon::WARNING, p.warning),
            };
            let row = ui.horizontal(|ui| {
                ui.label(RichText::new(glyph).color(c));
                ui.label(RichText::new(v.rule.title()).strong().color(p.text));
                ui.label(RichText::new(&v.message).color(p.weak));
            });
            let r = row.response.interact(Sense::click());
            if r.clicked() {
                focus = v.at;
            }
            if v.at.is_some() {
                r.on_hover_text("Click to show it on the board");
            }
        }
    });
    if let (Some(at), Some(cam)) = (focus, &mut app.pcb.camera) {
        cam.center = Pt::new(at[0], at[1]);
        cam.scale = cam.scale.max(20.0);
    }
}

/// Colours offered for nets.
const NET_PALETTE: &[[u8; 3]] = &[
    [255, 99, 71],
    [255, 165, 0],
    [255, 215, 0],
    [50, 205, 50],
    [0, 206, 209],
    [30, 144, 255],
    [186, 85, 211],
    [255, 105, 180],
];

/// Changes the board with undo; an error goes to the toast.
fn board_edit(app: &mut CircuiterApp, f: impl FnOnce(&mut Board)) {
    let Some(ws) = &mut app.workspace else { return };
    if let Err(e) = ws.edit_board(|_, _, b| {
        f(b.as_mut().ok_or("no board")?);
        Ok(())
    }) {
        app.error = Some(e);
    }
}

/// Every net, as Altium's PCB panel lists them: colour, class, pads, routed length
/// (flagged against its class's limits), vias and what's left to route. Click one to
/// see it on the board.
pub fn nets_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let p = style::palette(ui);
    let (Some(layout), Some(checked)) =
        (app.pcb.layout(), app.pcb.analysis.as_ref().and_then(|a| a.checked.as_ref()))
    else {
        ui.label(RichText::new("No board.").color(p.weak));
        return;
    };
    let nets = circuiter_pcb::nets::nets(layout, checked);
    let layout = layout.clone();
    let colours = layout.board.net_colours.clone();
    let classes: Vec<String> = layout.board.rules.classes.iter().map(|c| c.name.clone()).collect();
    let editable = app.workspace.as_ref().is_some_and(|ws| ws.editable());
    let mut focus: Option<String> = None;
    let mut recolour: Option<(String, Option<[u8; 3]>)> = None;
    let mut reclass: Option<(String, Option<String>)> = None;
    let mut net_colours = app.pcb.net_colours;
    ui.horizontal(|ui| {
        ui.checkbox(&mut net_colours, "Show net colours on routing  (F5)");
        if let Some(h) = &app.pcb.highlight {
            ui.label(RichText::new(format!("Showing {h}")).color(p.accent));
            if ui.small_button("Clear").clicked() {
                focus = Some(String::new());
            }
        }
    });
    app.pcb.net_colours = net_colours;
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        egui::Grid::new("pcb-nets").striped(true).num_columns(7).show(ui, |ui| {
            for h in ["", "Net", "Class", "Pads", "Routed", "Vias", "Unrouted"] {
                ui.label(RichText::new(h).small().color(p.weak));
            }
            ui.end_row();
            for n in &nets {
                // The colour swatch: click for the next colour, right-click for none.
                let c = colours.get(&n.name).copied();
                let (r, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
                match c {
                    Some([cr, cg, cb]) => {
                        ui.painter().rect_filled(r, 2.0, Color32::from_rgb(cr, cg, cb));
                    }
                    None => {
                        ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, p.weak), egui::StrokeKind::Inside);
                    }
                }
                if editable && resp.clicked() {
                    let k = c.and_then(|c| NET_PALETTE.iter().position(|x| *x == c)).map_or(0, |k| (k + 1) % NET_PALETTE.len());
                    recolour = Some((n.name.clone(), Some(NET_PALETTE[k])));
                }
                if editable && resp.secondary_clicked() {
                    recolour = Some((n.name.clone(), None));
                }
                let name = RichText::new(&n.name).strong().color(if app.pcb.highlight.as_deref() == Some(&n.name) { p.accent } else { p.text });
                if ui.add(egui::Label::new(name).sense(Sense::click())).on_hover_text("Show it on the board").clicked() {
                    focus = Some(n.name.clone());
                }
                ui.add_enabled_ui(editable, |ui| {
                    egui::ComboBox::from_id_salt(("net-class", &n.name))
                        .selected_text(n.class.clone().unwrap_or_else(|| "Default".into()))
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(n.class.is_none(), "Default").clicked() {
                                reclass = Some((n.name.clone(), None));
                            }
                            for c in &classes {
                                if ui.selectable_label(n.class.as_deref() == Some(c), c).clicked() {
                                    reclass = Some((n.name.clone(), Some(c.clone())));
                                }
                            }
                        });
                });
                ui.label(n.pads.to_string());
                let length = RichText::new(format!("{:.2} mm", n.routed));
                ui.label(match n.length {
                    Some(std::cmp::Ordering::Less) => length.background_color(Color32::from_rgb(120, 110, 20)),
                    Some(std::cmp::Ordering::Greater) => length.background_color(Color32::from_rgb(130, 30, 30)),
                    _ => length,
                })
                .on_hover_text("Tracks, plus the board's thickness for each via. Yellow: shorter than its class allows; red: longer");
                ui.label(n.vias.to_string());
                ui.label(if n.unrouted == 0 { RichText::new("—").color(p.weak) } else { RichText::new(n.unrouted.to_string()).color(p.warning) });
                ui.end_row();
            }
        });
    });
    if let Some(net) = focus {
        if net.is_empty() {
            app.pcb.highlight = None;
        } else {
            // Centre on its copper.
            let pts: Vec<P> = layout
                .pads
                .iter()
                .filter(|pad| pad.net.as_deref() == Some(net.as_str()))
                .map(|pad| pad.center)
                .collect();
            if let (Some(cam), false) = (&mut app.pcb.camera, pts.is_empty()) {
                let n = pts.len() as f64;
                cam.center = Pt::new(
                    pts.iter().map(|q| q[0]).sum::<f64>() / n,
                    pts.iter().map(|q| q[1]).sum::<f64>() / n,
                );
            }
            app.pcb.highlight = Some(net);
        }
    }
    if let Some((net, c)) = recolour {
        board_edit(app, |b| match c {
            Some(c) => drop(b.net_colours.insert(net, c)),
            None => drop(b.net_colours.remove(&net)),
        });
        app.pcb.net_colours = true;
    }
    if let Some((net, class)) = reclass {
        board_edit(app, |b| {
            for c in &mut b.rules.classes {
                c.nets.retain(|n| *n != net);
            }
            if let Some(c) = class.and_then(|c| b.rules.classes.iter_mut().find(|x| x.name == c)) {
                c.nets.push(net);
            }
        });
    }
}

/// Net classes: their clearance, track width, via, length limits and impedance profile.
pub fn classes_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let p = style::palette(ui);
    let Some(layout) = app.pcb.layout() else { return };
    let board = layout.board.clone();
    let stack = board.stack();
    let editable = app.workspace.as_ref().is_some_and(|ws| ws.editable());
    let mut rules = board.rules.clone();
    let mut add = false;
    let mut remove = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Nets not in a class follow the board's design rules (Properties). A class sets its own; assign nets in the Nets tab.").color(p.weak));
        if ui.add_enabled(editable, egui::Button::new(format!("{}  New Class", icon::PLUS))).clicked() {
            add = true;
        }
    });
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("net-classes").striped(true).num_columns(10).show(ui, |ui| {
                for h in [
                    "Class",
                    "Nets",
                    "Clearance",
                    "Track",
                    "Via",
                    "Drill",
                    "Min length",
                    "Max length",
                    "Impedance",
                    "",
                ] {
                    ui.label(RichText::new(h).small().color(p.weak));
                }
                ui.end_row();
                for (k, c) in rules.classes.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(80.0));
                    ui.label(c.nets.len().to_string()).on_hover_text(c.nets.join(", "));
                    for v in [
                        &mut c.clearance,
                        &mut c.track_width,
                        &mut c.via_diameter,
                        &mut c.via_drill,
                    ] {
                        let mut mm = v.mm();
                        if ui
                            .add(
                                egui::DragValue::new(&mut mm)
                                    .range(0.05..=20.0)
                                    .speed(0.01)
                                    .max_decimals(3),
                            )
                            .changed()
                        {
                            *v = Length::from_mm(mm);
                        }
                    }
                    for v in [&mut c.min_length, &mut c.max_length] {
                        let mut on = v.is_some();
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut on, "").changed() {
                                *v = on.then(|| Length::from_mm(50.0));
                            }
                            if let Some(l) = v {
                                let mut mm = l.mm();
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut mm)
                                            .range(0.0..=10000.0)
                                            .speed(0.5)
                                            .suffix(" mm"),
                                    )
                                    .changed()
                                {
                                    *l = Length::from_mm(mm);
                                }
                            }
                        });
                    }
                    egui::ComboBox::from_id_salt(("class-profile", k))
                        .selected_text(c.impedance.clone().unwrap_or_else(|| "—".into()))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(c.impedance.is_none(), "—").clicked() {
                                c.impedance = None;
                            }
                            for prof in &stack.profiles {
                                if ui
                                    .selectable_label(
                                        c.impedance.as_deref() == Some(&prof.name),
                                        &prof.name,
                                    )
                                    .clicked()
                                {
                                    c.impedance = Some(prof.name.clone());
                                    // The profile sizes the class's tracks.
                                    if let Some(w) =
                                        circuiter_pcb::impedance::profile_width(&stack, prof)
                                    {
                                        c.track_width =
                                            Length::from_mm((w * 1000.0).round() / 1000.0);
                                    }
                                }
                            }
                        });
                    if ui.small_button(icon::TRASH).on_hover_text("Delete the class").clicked() {
                        remove = Some(k);
                    }
                    ui.end_row();
                }
            });
        });
    });
    if add {
        let name = (1..)
            .map(|k| format!("Class{k}"))
            .find(|n| !rules.classes.iter().any(|c| c.name == *n))
            .expect("a name");
        rules.classes.push(circuiter_model::board::NetClass::new(&name, &rules));
    }
    if let Some(k) = remove {
        rules.classes.remove(k);
    }
    if rules != board.rules {
        board_edit(app, |b| b.rules = rules);
    }
}

/// The layer stack (Altium's Layer Stack Manager): each layer's material and
/// thickness, and impedance profiles with the track widths they need.
pub fn stackup_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    use circuiter_model::board::{ImpedanceProfile, StackKind};
    let p = style::palette(ui);
    let Some(layout) = app.pcb.layout() else { return };
    let board = layout.board.clone();
    let editable = app.workspace.as_ref().is_some_and(|ws| ws.editable());
    let mut stack = board.stack();
    let before = stack.clone();
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} copper layers · {:.3} mm in all",
                board.copper_layers,
                stack.thickness()
            ))
            .strong(),
        );
        if ui
            .add_enabled(editable, egui::Button::new("Standard FR-4"))
            .on_hover_text("1 oz copper, FR-4 cores and prepregs, the board's thickness")
            .clicked()
        {
            reset = true;
        }
    });
    egui::ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("stack").striped(true).num_columns(5).show(ui, |ui| {
                for h in ["", "Layer", "Type", "Thickness", "εr"] {
                    ui.label(RichText::new(h).small().color(p.weak));
                }
                ui.end_row();
                for l in &mut stack.layers {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                    let c = match l.kind {
                        StackKind::Copper => Color32::from_rgb(200, 120, 60),
                        StackKind::SolderMask => Color32::from_rgb(40, 140, 70),
                        _ => Color32::from_rgb(190, 180, 120),
                    };
                    ui.painter().rect_filled(r, 1.0, c);
                    ui.label(&l.name);
                    ui.label(match l.kind {
                        StackKind::Copper => "Copper",
                        StackKind::Core => "Core",
                        StackKind::Prepreg => "Prepreg",
                        StackKind::SolderMask => "Solder mask",
                    });
                    let mut mm = l.thickness.mm();
                    if ui.add(egui::DragValue::new(&mut mm).range(0.005..=5.0).speed(0.005).max_decimals(3).suffix(" mm")).changed() {
                        l.thickness = Length::from_mm(mm);
                    }
                    if l.kind == StackKind::Copper {
                        ui.label("");
                    } else {
                        ui.add(egui::DragValue::new(&mut l.er).range(1.0..=12.0).speed(0.01).max_decimals(2));
                    }
                    ui.end_row();
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Impedance profiles").strong());
            if ui.add_enabled(editable, egui::Button::new(format!("{}  Add", icon::PLUS))).clicked() {
                let name = (1..).map(|k| format!("Z{k}")).find(|n| !stack.profiles.iter().any(|x| x.name == *n)).expect("a name");
                stack.profiles.push(ImpedanceProfile { name, ohms: 50.0, layer: Layer::FCu, differential: false, gap: Length::from_mm(0.2) });
            }
        });
        let copper = board.copper();
        let mut remove = None;
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("profiles").striped(true).num_columns(7).show(ui, |ui| {
                for h in ["Name", "Target", "Layer", "Pair", "Gap", "Width", ""] {
                    ui.label(RichText::new(h).small().color(p.weak));
                }
                ui.end_row();
                let snapshot = stack.clone();
                for (k, prof) in stack.profiles.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut prof.name).desired_width(60.0));
                    ui.add(egui::DragValue::new(&mut prof.ohms).range(10.0..=200.0).speed(0.5).suffix(" Ω"));
                    egui::ComboBox::from_id_salt(("profile-layer", k)).selected_text(prof.layer.name()).show_ui(ui, |ui| {
                        for &l in &copper {
                            ui.selectable_value(&mut prof.layer, l, l.name());
                        }
                    });
                    ui.checkbox(&mut prof.differential, "");
                    let mut gap = prof.gap.mm();
                    if ui.add_enabled(prof.differential, egui::DragValue::new(&mut gap).range(0.05..=5.0).speed(0.01).suffix(" mm")).changed() {
                        prof.gap = Length::from_mm(gap);
                    }
                    match circuiter_pcb::impedance::profile_width(&snapshot, prof) {
                        Some(w) => ui.label(RichText::new(format!("{w:.3} mm")).strong().color(p.go)),
                        None => ui.label(RichText::new("out of reach").color(p.error)),
                    };
                    if ui.small_button(icon::TRASH).clicked() {
                        remove = Some(k);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(k) = remove {
            stack.profiles.remove(k);
        }
        ui.label(
            RichText::new("Widths by IPC-2141's formulas: microstrip on outer layers, stripline on inner ones. Within a few per cent; confirm with your board house. Give a net class a profile (Net Classes tab) to size its tracks.")
                .small()
                .color(p.weak),
        );
    });
    if reset {
        stack = circuiter_model::board::Stackup {
            profiles: stack.profiles,
            ..circuiter_model::board::Stackup::standard(board.copper_layers, board.thickness.mm())
        };
    }
    if stack != before {
        let thickness = Length::from_mm(stack.thickness());
        board_edit(app, |b| {
            b.thickness = thickness;
            b.stackup = Some(stack);
        });
    }
}

/// Differential pairs (Altium's Differential Pairs Editor): two nets routed side by
/// side at a set width and gap, their lengths kept within a skew.
pub fn pairs_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    use circuiter_model::board::DiffPair;
    let p = style::palette(ui);
    let Some(layout) = app.pcb.layout().cloned() else { return };
    let board = &layout.board;
    let editable = app.workspace.as_ref().is_some_and(|ws| ws.editable());
    let nets: Vec<String> = layout.nets.iter().cloned().collect();
    let mut pairs = board.pairs.clone();
    let mut route = None;
    let suggested: Vec<DiffPair> = DiffPair::from_names(&nets)
        .into_iter()
        .filter(|s| !pairs.iter().any(|q| q.positive == s.positive || q.negative == s.negative))
        .map(|mut s| {
            s.width = board.rules.track_width;
            s.gap = board.rules.clearance;
            s
        })
        .collect();
    ui.label(RichText::new("Each pair is routed as one: both nets side by side, a gap apart, from one pair of pads to the other. Skew is how much longer one is.").color(p.weak));
    ui.horizontal(|ui| {
        let hint = if suggested.is_empty() {
            "No more nets named as pairs (X_P/X_N, X+/X−, XP/XN)".to_owned()
        } else {
            suggested.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ")
        };
        if ui
            .add_enabled(
                editable && !suggested.is_empty(),
                egui::Button::new(format!(
                    "{}  From Net Names ({})",
                    icon::MAGIC_WAND,
                    suggested.len()
                )),
            )
            .on_hover_text(hint)
            .clicked()
        {
            pairs.extend(suggested.iter().cloned());
        }
        if ui
            .add_enabled(
                editable && nets.len() >= 2,
                egui::Button::new(format!("{}  New Pair", icon::PLUS)),
            )
            .clicked()
        {
            let name = (1..)
                .map(|k| format!("Pair{k}"))
                .find(|n| !pairs.iter().any(|q| q.name == *n))
                .expect("a name");
            pairs.push(DiffPair {
                name,
                positive: nets[0].clone(),
                negative: nets[1].clone(),
                width: board.rules.track_width,
                gap: board.rules.clearance,
                max_skew: None,
            });
        }
    });
    let mut remove = None;
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("diff-pairs").striped(true).num_columns(9).show(ui, |ui| {
                for h in [
                    "Pair", "Positive", "Negative", "Width", "Gap", "Lengths", "Skew", "Max skew",
                    "",
                ] {
                    ui.label(RichText::new(h).small().color(p.weak));
                }
                ui.end_row();
                for (k, pair) in pairs.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut pair.name).desired_width(70.0));
                    for (salt, net) in
                        [("pair-p", &mut pair.positive), ("pair-n", &mut pair.negative)]
                    {
                        egui::ComboBox::from_id_salt((salt, k)).selected_text(net.clone()).show_ui(
                            ui,
                            |ui| {
                                for n in &nets {
                                    ui.selectable_value(net, n.clone(), n);
                                }
                            },
                        );
                    }
                    for v in [&mut pair.width, &mut pair.gap] {
                        let mut mm = v.mm();
                        if ui
                            .add(
                                egui::DragValue::new(&mut mm)
                                    .range(0.05..=5.0)
                                    .speed(0.01)
                                    .max_decimals(3),
                            )
                            .changed()
                        {
                            *v = Length::from_mm(mm);
                        }
                    }
                    let (a, b) = (
                        circuiter_pcb::nets::routed_length(board, &pair.positive),
                        circuiter_pcb::nets::routed_length(board, &pair.negative),
                    );
                    ui.label(format!("{a:.2} / {b:.2} mm"));
                    let skew = (a - b).abs();
                    let over = pair.max_skew.is_some_and(|m| skew > m.mm()) && a > 0.0 && b > 0.0;
                    let text = RichText::new(format!("{skew:.3} mm"));
                    ui.label(if over { text.color(p.error) } else { text });
                    ui.horizontal(|ui| {
                        let mut on = pair.max_skew.is_some();
                        if ui.checkbox(&mut on, "").changed() {
                            pair.max_skew = on.then(|| Length::from_mm(0.5));
                        }
                        if let Some(m) = &mut pair.max_skew {
                            let mut mm = m.mm();
                            if ui
                                .add(
                                    egui::DragValue::new(&mut mm)
                                        .range(0.0..=100.0)
                                        .speed(0.01)
                                        .suffix(" mm"),
                                )
                                .changed()
                            {
                                *m = Length::from_mm(mm);
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("Route")
                            .on_hover_text(
                                "Route the pair together (replacing what's routed of it)",
                            )
                            .clicked()
                        {
                            route = Some(k);
                        }
                        if ui.small_button(icon::TRASH).on_hover_text("Delete the pair").clicked() {
                            remove = Some(k);
                        }
                    });
                    ui.end_row();
                }
            });
        });
    });
    if let Some(k) = remove {
        pairs.remove(k);
    }
    if pairs != board.pairs {
        board_edit(app, |b| b.pairs = pairs.clone());
    }
    if let Some(k) = route {
        app.pcb_route_pair(&pairs[k]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::Project;
    use std::path::Path;

    /// A ratsnest line whose straight route runs into another net's copper: walkaround
    /// finds a clear way round, stop-at-obstacle stops short, ignore goes straight over.
    #[test]
    fn walkaround_goes_round_what_stop_stops_at() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
        let project = Project::load(&root).unwrap();
        let mut design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let id = CellId::new("analog_lab", "blinker");
        let board = sync::new_board(&design, &id).unwrap();
        design.cell_mut(&id).unwrap().board = Some(board);
        let mut view = PcbView::default();
        view.update(&design, &id, 1);
        let layout = view.layout().unwrap().clone();
        let checked = view.analysis.as_ref().unwrap().checked.as_ref().unwrap();
        let width = layout.board.rules.track_width.mm();
        let blocked = checked
            .connectivity
            .ratsnest
            .iter()
            .find(|(net, a, b)| {
                route_margin(
                    &layout,
                    net,
                    &legs(*a, *b, Corner::Diagonal, false),
                    width,
                    Layer::FCu,
                ) < -1e-6
            })
            .cloned()
            .expect("a blocked straight route");
        let (net, a, b) = blocked;
        view.mode = RouteMode::Walkaround;
        let (path, stuck) = view.route_to(&net, width, Layer::FCu, a, b);
        assert!(!stuck, "walkaround finds a way");
        assert!(
            route_margin(&layout, &net, &path, width, Layer::FCu) >= -1e-6,
            "clear of other nets: {path:?}"
        );
        let end = *path.last().unwrap();
        assert!((end[0] - b[0]).hypot(end[1] - b[1]) < 1e-6, "it gets there");
        // Push and shove: the straight route with tracks moved aside, or else around.
        view.mode = RouteMode::Shove;
        let (path, stuck) = view.route_to(&net, width, Layer::FCu, a, b);
        assert!(!stuck, "shoving or walking round finds a way");
        let mut after = layout.clone();
        if let Some(moved) = &view.shoved {
            after.board = moved.clone();
        }
        assert!(route_margin(&after, &net, &path, width, Layer::FCu) >= -1e-6, "{path:?}");
        view.mode = RouteMode::Stop;
        let (cut, short) = view.route_to(&net, width, Layer::FCu, a, b);
        assert!(short);
        let end = *cut.last().unwrap();
        assert!((end[0] - b[0]).hypot(end[1] - b[1]) > 0.1, "stopped short");
        view.mode = RouteMode::Ignore;
        let (_, over) = view.route_to(&net, width, Layer::FCu, a, b);
        assert!(over, "ignore says it's blocked but goes anyway");
    }
}
