//! Analyses: operating point, DC sweep and transient, built on one Newton-Raphson
//! solver over the modified nodal equations.

use crate::circuit::{Circuit, Element, Problem};
use crate::device::{
    Layout, Stamp, VT, bulk_junction, conductance, current, depletion, fetlim, limvds,
    mos1_channel, pnjlim,
};
use crate::solve::Collect;
use crate::sparse::{Lu, Pattern, Scalar, SparseSystem, minimum_degree};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Lets another thread follow a running analysis and stop it.
#[derive(Debug, Default)]
pub struct Control {
    cancel: AtomicBool,
    /// Fraction done, as `f64` bits.
    progress: AtomicU64,
}

impl Control {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// How far the analysis has got, from 0 to 1.
    pub fn progress(&self) -> f64 {
        f64::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn set_progress(&self, fraction: f64) {
        self.progress.store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

/// Solver settings. The defaults match SPICE's.
#[derive(Debug, Clone)]
pub struct Options {
    /// Relative tolerance on voltages, currents and charges.
    pub reltol: f64,
    /// Absolute voltage tolerance (V).
    pub vntol: f64,
    /// Absolute current tolerance (A).
    pub abstol: f64,
    /// Absolute charge tolerance (C).
    pub chgtol: f64,
    /// How much truncation error the time step may allow, relative to the tolerances.
    pub trtol: f64,
    /// Conductance across every junction, which keeps reverse-biased junctions solvable.
    pub gmin: f64,
    /// Conductance from every node to ground: the leakage of a real board. It keeps
    /// floating nodes solvable, so a circuit without ground behaves like a real one (it
    /// doesn't work) rather than failing to simulate. Zero turns it off.
    pub leakage: f64,
    /// Newton iterations for an operating point.
    pub itl1: usize,
    /// Newton iterations per time step.
    pub itl4: usize,
    /// A transient run stops with an error after this many time points.
    pub max_points: usize,
    /// Start a transient run from power-on: every node at 0 V and every capacitor
    /// empty, with the sources switched on at t = 0, as when a real circuit is turned on.
    /// Otherwise (SPICE's default) the run starts from the DC operating point, as if the
    /// circuit had been on forever.
    pub power_on: bool,
    /// Progress reporting and cancellation, for running on another thread.
    pub control: Option<Arc<Control>>,
}

impl Options {
    fn progress(&self, fraction: f64) -> Result<(), SimError> {
        if let Some(c) = &self.control {
            if c.is_cancelled() {
                return Err(SimError {
                    message: "cancelled".into(),
                    nodes: Vec::new(),
                    devices: Vec::new(),
                });
            }
            c.set_progress(fraction);
        }
        Ok(())
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            reltol: 1e-3,
            vntol: 1e-6,
            abstol: 1e-12,
            chgtol: 1e-14,
            trtol: 7.0,
            gmin: 1e-12,
            leakage: 1e-12,
            itl1: 100,
            itl4: 10,
            max_points: 2_000_000,
            power_on: false,
            control: None,
        }
    }
}

/// Why a simulation could not finish, naming the nodes or devices to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct SimError {
    pub message: String,
    pub nodes: Vec<String>,
    pub devices: Vec<String>,
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SimError {}

/// The operating point: every node voltage and branch current, and what each device
/// is doing there.
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    /// By node index; `voltages[0]` is ground.
    pub voltages: Vec<f64>,
    /// By branch (voltage sources and inductors), SPICE's sign: positive current flows
    /// into the source's `+` terminal.
    pub branch_currents: Vec<f64>,
    pub devices: Vec<DeviceOp>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceOp {
    pub name: String,
    /// Current flowing into the device at each terminal, in the card's terminal order.
    pub currents: Vec<f64>,
    /// Power absorbed (negative when the device delivers power, as a source does).
    pub power: f64,
}

impl OpResult {
    pub fn voltage(&self, c: &Circuit, node: &str) -> Option<f64> {
        c.node(node).map(|k| self.voltages[k])
    }

    pub fn device(&self, name: &str) -> Option<&DeviceOp> {
        self.devices.iter().find(|d| d.name.eq_ignore_ascii_case(name))
    }
}

/// Waveforms from a transient analysis: every unknown at every accepted time point.
#[derive(Debug, Clone, PartialEq)]
pub struct TranResult {
    pub time: Vec<f64>,
    /// Node voltages by node index, then branch currents: one row per time point.
    pub values: Vec<Vec<f64>>,
    nodes: usize,
    /// Steps tried and thrown away (too much error, or no convergence).
    pub rejected: usize,
}

impl TranResult {
    /// A node's voltage over time.
    pub fn voltage(&self, c: &Circuit, node: &str) -> Option<Vec<f64>> {
        let k = c.node(node)?;
        Some(self.values.iter().map(|row| if k == 0 { 0.0 } else { row[k - 1] }).collect())
    }

    /// A voltage source's or inductor's current over time.
    pub fn current(&self, c: &Circuit, branch: &str) -> Option<Vec<f64>> {
        let j = c.branch_of(branch)?;
        Some(self.values.iter().map(|row| row[self.nodes - 1 + j]).collect())
    }

    /// Linear interpolation of `signal` at time `t`.
    pub fn sample(&self, signal: &[f64], t: f64) -> f64 {
        let k = self.time.partition_point(|&x| x < t);
        if k == 0 {
            return signal[0];
        }
        if k >= self.time.len() {
            return *signal.last().expect("not empty");
        }
        let (t0, t1) = (self.time[k - 1], self.time[k]);
        let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
        signal[k - 1] + f * (signal[k] - signal[k - 1])
    }
}

/// One charge-storage element's state: charge (or flux) and the current (or voltage)
/// it carries.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Slot {
    q: f64,
    i: f64,
    /// For Meyer gate capacitances, which SPICE integrates step by step: the voltage
    /// across the capacitance and its half value at this point.
    v: f64,
    c: f64,
}

struct Ctx<'a> {
    /// During a transient run, the time (sources follow their waveforms).
    time: Option<f64>,
    tstep: f64,
    tstop: f64,
    /// Source stepping: every independent source is scaled by this.
    scale: f64,
    /// Integration coefficient (1/h or 2/h); zero in DC.
    ag0: f64,
    trap: bool,
    hist: &'a [Slot],
    /// First iteration of an operating point: junctions start at a sensible guess.
    init: bool,
    gmin: f64,
}

enum Fail {
    Singular(usize),
    NoConvergence(usize),
}

struct Engine<'c> {
    c: &'c Circuit,
    o: &'c Options,
    l: Layout,
    n: usize,
    /// The sparse structure, fill-reducing order and latest factorisation, kept across
    /// Newton iterations and time steps so most solves only refactor.
    solver: RefCell<Option<Solver>>,
    /// The transient run's print step and stop time, which set unspecified waveform
    /// times when sources are evaluated outside the stepping loop.
    tstep: std::cell::Cell<f64>,
    tstop: std::cell::Cell<f64>,
}

struct Solver {
    pattern: Pattern,
    order: Vec<usize>,
    lu: Option<Lu<f64>>,
}

impl Solver {
    fn new(pattern: Pattern) -> Self {
        let order = minimum_degree(&pattern);
        Solver { pattern, order, lu: None }
    }
}

/// Relative size a pivot needs, against the largest in its column, to be kept. Larger
/// is more stable (less error growth through long chains), smaller keeps more sparsity.
const PIVOT_THRESHOLD: f64 = 0.1;

/// Per-device memory between Newton iterations, for limiting.
/// Diodes keep `[vd, id, gd]`; MOSFETs `[vgs, vds, vbs, von, id, gm, gds, gmbs, gbd, gbs,
/// cbulk, mode]`, all in normalised (n-channel) form.
type Limits = Vec<[f64; 12]>;

impl<'c> Engine<'c> {
    fn new(c: &'c Circuit, o: &'c Options) -> Self {
        let l = Layout { nodes: c.nodes.len() };
        Engine {
            c,
            o,
            l,
            n: l.size(c.branches.len()),
            solver: RefCell::new(None),
            tstep: std::cell::Cell::new(1.0),
            tstop: std::cell::Cell::new(1.0),
        }
    }

    /// An empty system in the current sparse structure, which is first taken from
    /// stamping every device in a transient context.
    fn system(&self) -> SparseSystem {
        let mut guard = self.solver.borrow_mut();
        let solver = guard.get_or_insert_with(|| {
            let hist = vec![Slot::default(); self.c.slots];
            let ctx = Ctx {
                time: Some(0.0),
                tstep: 1.0,
                tstop: 1.0,
                scale: 1.0,
                ag0: 1.0,
                trap: false,
                hist: &hist,
                init: true,
                gmin: self.o.gmin,
            };
            let mut col = Collect::default();
            let mut lim = vec![[0.0; 12]; self.c.devices.len()];
            let mut now = hist.clone();
            self.load(&vec![0.0; self.n], &ctx, &mut lim, &mut now, &mut col);
            Solver::new(Pattern::new(self.n, col.a.iter().map(|e| (e.0, e.1))))
        });
        SparseSystem::new(solver.pattern.clone())
    }

    /// Adds entries a stamp needed that the structure lacked.
    fn grow(&self, missing: &[(usize, usize)]) {
        let mut guard = self.solver.borrow_mut();
        let old = guard.take().expect("system() created it");
        let entries: Vec<(usize, usize)> =
            old.pattern.entries().chain(missing.iter().copied()).collect();
        *guard = Some(Solver::new(Pattern::new(self.n, entries)));
    }

    /// Solves the assembled system, refactoring the previous factorisation when its
    /// pivots still serve.
    fn solve(&self, sys: &SparseSystem) -> Result<Vec<f64>, usize> {
        let mut guard = self.solver.borrow_mut();
        let s = guard.as_mut().expect("system() created it");
        let mut b = sys.b.clone();
        if let Some(lu) = &mut s.lu {
            if lu.refactor(&s.pattern, &sys.values) {
                lu.solve(&mut b);
                return Ok(b);
            }
        }
        s.lu = None;
        let lu = Lu::factor(&s.pattern, &sys.values, &s.order, PIVOT_THRESHOLD)?;
        lu.solve(&mut b);
        s.lu = Some(lu);
        Ok(b)
    }

    fn row_name(&self, row: usize) -> String {
        if row + 1 < self.c.nodes.len() {
            self.c.nodes[row + 1].clone()
        } else {
            format!("current of {}", self.c.branches[row + 1 - self.c.nodes.len()])
        }
    }

    /// The KCL and branch-equation entries every voltage-defining element shares:
    /// branch current leaves `p` and enters `n`, and the branch row starts `v(p) - v(n)`.
    fn branch_terminals(s: &mut dyn Stamp, l: Layout, br: usize, p: usize, n: usize) {
        if let Some(rp) = l.node(p) {
            s.a(rp, br, 1.0);
            s.a(br, rp, 1.0);
        }
        if let Some(rn) = l.node(n) {
            s.a(rn, br, -1.0);
            s.a(br, rn, -1.0);
        }
    }

    fn integrate(ctx: &Ctx, slot: usize, q: f64, c: f64) -> (f64, f64) {
        if ctx.ag0 == 0.0 {
            return (0.0, 0.0);
        }
        let h = ctx.hist[slot];
        let i = ctx.ag0 * (q - h.q) - if ctx.trap { h.i } else { 0.0 };
        (i, ctx.ag0 * c)
    }

    fn source_value(ctx: &Ctx, s: &crate::device::Source) -> f64 {
        ctx.scale
            * match ctx.time {
                Some(t) => s.at(t, ctx.tstep, ctx.tstop),
                None => s.dc(),
            }
    }

    /// Stamps device `di` linearised about `x`. Returns whether limiting changed its
    /// junction voltages, in which case the iteration can't count as converged.
    fn load_device(
        &self,
        di: usize,
        x: &[f64],
        ctx: &Ctx,
        lim: &mut [f64; 12],
        now: &mut [Slot],
        s: &mut dyn Stamp,
    ) -> bool {
        let l = self.l;
        let v = |k: usize| if k == 0 { 0.0 } else { x[k - 1] };
        let dev = &self.c.devices[di];
        let mut limited = false;
        match &dev.element {
            Element::Resistor { a, b, g } => conductance(s, l, *a, *b, *g),
            Element::Capacitor { a, b, c } => {
                let vd = v(*a) - v(*b);
                let q = c * vd;
                let (i, geq) = Self::integrate(ctx, dev.slot, q, *c);
                now[dev.slot] = Slot { q, i, ..Slot::default() };
                if ctx.ag0 > 0.0 {
                    current(s, l, *a, *b, i, &[(*a, *b, geq, vd)]);
                }
            }
            Element::Inductor { a, b, l: ind, branch, mutual } => {
                let br = l.branch(*branch);
                if let Some(ra) = l.node(*a) {
                    s.a(ra, br, 1.0);
                    s.a(br, ra, 1.0);
                }
                if let Some(rb) = l.node(*b) {
                    s.a(rb, br, -1.0);
                    s.a(br, rb, -1.0);
                }
                // Flux linkage, including what coupled inductors contribute.
                let mut flux = ind * x[br];
                for &(other, m) in mutual {
                    flux += m * x[l.branch(other)];
                }
                if ctx.ag0 > 0.0 {
                    for &(other, m) in mutual {
                        s.a(br, l.branch(other), -ctx.ag0 * m);
                    }
                    // v = dφ/dt, integrated like a charge: v = ag0 φ + history.
                    let h = ctx.hist[dev.slot];
                    let hist = -ctx.ag0 * h.q - if ctx.trap { h.i } else { 0.0 };
                    s.a(br, br, -ctx.ag0 * ind);
                    s.b(br, hist);
                    now[dev.slot] = Slot { q: flux, i: ctx.ag0 * flux + hist, ..Slot::default() };
                } else {
                    now[dev.slot] = Slot { q: flux, ..Slot::default() };
                }
            }
            Element::Vsource { p, n, branch, source } => {
                let br = l.branch(*branch);
                if let Some(rp) = l.node(*p) {
                    s.a(rp, br, 1.0);
                    s.a(br, rp, 1.0);
                }
                if let Some(rn) = l.node(*n) {
                    s.a(rn, br, -1.0);
                    s.a(br, rn, -1.0);
                }
                s.b(br, Self::source_value(ctx, source));
            }
            Element::Isource { p, n, source } => {
                current(s, l, *p, *n, Self::source_value(ctx, source), &[]);
            }
            Element::Vccs { p, n, cp, cn, gm } => {
                current(s, l, *p, *n, 0.0, &[(*cp, *cn, *gm, 0.0)]);
            }
            Element::Cccs { p, n, control, gain } => {
                let ctrl = l.branch(*control);
                if let Some(r) = l.node(*p) {
                    s.a(r, ctrl, *gain);
                }
                if let Some(r) = l.node(*n) {
                    s.a(r, ctrl, -*gain);
                }
            }
            Element::Vcvs { p, n, cp, cn, gain, branch } => {
                let br = l.branch(*branch);
                Self::branch_terminals(s, l, br, *p, *n);
                for (node, sign) in [(*cp, -1.0), (*cn, 1.0)] {
                    if let Some(r) = l.node(node) {
                        s.a(br, r, sign * gain);
                    }
                }
            }
            Element::Failed { branch, short } => {
                if let Some(b) = branch {
                    let br = l.branch(*b);
                    s.a(br, br, 1.0);
                }
                if let Some((a, b, g)) = short {
                    conductance(s, l, *a, *b, *g);
                }
            }
            Element::Switch { p, n, control, ron, roff, threshold, hysteresis, initially_on } => {
                let c = match *control {
                    crate::circuit::SwitchControl::Voltage(a, b) => v(a) - v(b),
                    crate::circuit::SwitchControl::Current(br) => x[l.branch(br)],
                };
                // The state at the last accepted point; in DC, the initial state.
                let before = if ctx.ag0 > 0.0 { ctx.hist[dev.slot].q > 0.5 } else { *initially_on };
                let on = if c > threshold + hysteresis {
                    true
                } else if c < threshold - hysteresis {
                    false
                } else {
                    before
                };
                // A change since the last iteration means the solution isn't settled.
                let state = if on { 1.0 } else { 0.0 };
                if !ctx.init && lim[0] != state {
                    limited = true;
                }
                lim[0] = state;
                now[dev.slot] = Slot { q: state, ..Slot::default() };
                conductance(s, l, *p, *n, 1.0 / if on { *ron } else { *roff });
            }
            Element::Poly { p, n, branch, inputs, poly } => {
                // Linearised: out = y(x0) + Σ ∂y/∂xᵢ (xᵢ - xᵢ0).
                let col = |input: &crate::circuit::PolyInput| match *input {
                    crate::circuit::PolyInput::Voltage(a, b) => (Some(a), Some(b), None),
                    crate::circuit::PolyInput::Current(br) => (None, None, Some(l.branch(br))),
                };
                let xs: Vec<f64> = inputs
                    .iter()
                    .map(|input| match col(input) {
                        (Some(a), Some(b), _) => v(a) - v(b),
                        (_, _, Some(br)) => x[br],
                        _ => 0.0,
                    })
                    .collect();
                let (y, grad) = poly.eval(&xs);
                let y0 = y - grad.iter().zip(&xs).map(|(g, x)| g * x).sum::<f64>();
                match branch {
                    Some(b) => {
                        let br = l.branch(*b);
                        Self::branch_terminals(s, l, br, *p, *n);
                        for (input, g) in inputs.iter().zip(&grad) {
                            match col(input) {
                                (Some(a), Some(b), _) => {
                                    if let Some(r) = l.node(a) {
                                        s.a(br, r, -g);
                                    }
                                    if let Some(r) = l.node(b) {
                                        s.a(br, r, *g);
                                    }
                                }
                                (_, _, Some(c)) => s.a(br, c, -g),
                                _ => {}
                            }
                        }
                        s.b(br, y0);
                    }
                    None => {
                        let mut controls = Vec::new();
                        for (input, g) in inputs.iter().zip(&grad) {
                            match col(input) {
                                (Some(a), Some(b), _) => controls.push((a, b, *g, 0.0)),
                                (_, _, Some(c)) => {
                                    if let Some(r) = l.node(*p) {
                                        s.a(r, c, *g);
                                    }
                                    if let Some(r) = l.node(*n) {
                                        s.a(r, c, -g);
                                    }
                                }
                                _ => {}
                            }
                        }
                        current(s, l, *p, *n, y0, &controls);
                    }
                }
            }
            Element::Ccvs { p, n, control, r, branch } => {
                let br = l.branch(*branch);
                Self::branch_terminals(s, l, br, *p, *n);
                s.a(br, l.branch(*control), -*r);
            }
            Element::Diode(d) => {
                let vraw = v(d.junction) - v(d.cathode);
                let vd = if ctx.init {
                    d.vcrit()
                } else {
                    let [vd_o, id_o, gd_o, ..] = *lim;
                    // The current must have settled too, not just the voltage
                    // (SPICE's DIOconvTest): its predicted change must be small.
                    let delta = gd_o * (vraw - vd_o);
                    if delta.abs()
                        > self.o.reltol * (id_o + delta).abs().max(id_o.abs()) + self.o.abstol
                    {
                        limited = true;
                    }
                    let (vl, changed) = d.limit(vraw, vd_o);
                    limited |= changed;
                    vl
                };
                let (id, gd) = d.iv(vd, ctx.gmin);
                *lim = [vd, id, gd, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
                current(s, l, d.junction, d.cathode, id, &[(d.junction, d.cathode, gd, vd)]);
                if d.model.rs > 0.0 {
                    conductance(s, l, d.anode, d.junction, d.area / d.model.rs);
                }
                let (q, cap) = d.charge(vd, id, gd);
                let (ic, gc) = Self::integrate(ctx, dev.slot, q, cap);
                now[dev.slot] = Slot { q, i: ic, ..Slot::default() };
                if ctx.ag0 > 0.0 {
                    current(s, l, d.junction, d.cathode, ic, &[(d.junction, d.cathode, gc, vd)]);
                }
            }
            Element::Bjt(q) => {
                let p = q.model.polarity;
                let vcrit = q.vcrit();
                let (mut vbe, mut vbc) = (p * (v(q.bi) - v(q.ei)), p * (v(q.bi) - v(q.ci)));
                if ctx.init {
                    (vbe, vbc) = (vcrit, 0.0);
                } else {
                    let [vbe_o, vbc_o, ic_o, ib_o, gm_o, go_o, gpi_o, gmu_o, ..] = *lim;
                    // Current convergence (SPICE's BJTconvTest).
                    let (dbe, dbc) = (vbe - vbe_o, vbc - vbc_o);
                    let dc = (gm_o + go_o) * dbe - (go_o + gmu_o) * dbc;
                    let db = gpi_o * dbe + gmu_o * dbc;
                    let tol =
                        |i: f64, d: f64| self.o.reltol * (i + d).abs().max(i.abs()) + self.o.abstol;
                    if dc.abs() > tol(ic_o, dc) || db.abs() > tol(ib_o, db) {
                        limited = true;
                    }
                    let (a, ca) = pnjlim(vbe, vbe_o, VT * q.model.nf, vcrit);
                    let (b, cb) = pnjlim(vbc, vbc_o, VT * q.model.nr, vcrit);
                    (vbe, vbc) = (a, b);
                    limited |= ca || cb;
                }
                let st = q.eval(vbe, vbc, ctx.gmin);
                *lim = [vbe, vbc, st.ic, st.ib, st.gm, st.go, st.gpi, st.gmu, 0.0, 0.0, 0.0, 0.0];
                let (ci, bi, ei) = (q.ci, q.bi, q.ei);
                let (pbe, pbc) = (p * vbe, p * vbc);
                current(
                    s,
                    l,
                    ci,
                    ei,
                    p * st.ic,
                    &[(bi, ei, st.gm + st.go, pbe), (bi, ci, -(st.gmu + st.go), pbc)],
                );
                current(s, l, bi, ei, p * st.ib, &[(bi, ei, st.gpi, pbe), (bi, ci, st.gmu, pbc)]);
                for (r, a, b) in
                    [(q.model.rc, q.c, ci), (q.model.rb, q.b, bi), (q.model.re, q.e, ei)]
                {
                    if r > 0.0 {
                        conductance(s, l, a, b, q.area / r);
                    }
                }
                let charges = [(st.qbe, st.cbe, ei, pbe), (st.qbc, st.cbc, ci, pbc)];
                for (k, (qn, c, other, vj)) in charges.into_iter().enumerate() {
                    let slot = dev.slot + k;
                    let qa = p * qn;
                    let (i, geq) = Self::integrate(ctx, slot, qa, c);
                    now[slot] = Slot { q: qa, i, ..Slot::default() };
                    if ctx.ag0 > 0.0 && c != 0.0 {
                        current(s, l, bi, other, i, &[(bi, other, geq, vj)]);
                    }
                }
            }
            Element::Mosfet(m) => {
                let model = &m.model;
                let p = model.polarity;
                let mut vbs = p * (v(m.b) - v(m.s));
                let mut vgs = p * (v(m.g) - v(m.s));
                let mut vds = p * (v(m.d) - v(m.s));
                let is = model.is * m.m;
                let vcrit = VT * (VT / (std::f64::consts::SQRT_2 * is)).ln();
                if ctx.init {
                    vbs = -1.0;
                    vgs = p * model.vto;
                    vds = 0.0;
                } else {
                    let [
                        vgs_o,
                        vds_o,
                        vbs_o,
                        von,
                        id_o,
                        gm_o,
                        gds_o,
                        gmbs_o,
                        gbd_o,
                        gbs_o,
                        cb_o,
                        mode_o,
                    ] = *lim;
                    // Current convergence (SPICE's MOS1convTest), from the unlimited
                    // voltages of the latest solution.
                    let (dvgs, dvds, dvbs) = (vgs - vgs_o, vds - vds_o, vbs - vbs_o);
                    let dvbd = dvbs - dvds;
                    let dch = if mode_o >= 0.0 {
                        gm_o * dvgs + gds_o * dvds + gmbs_o * dvbs
                    } else {
                        gm_o * (dvgs - dvds) - gds_o * dvds + gmbs_o * dvbd
                    };
                    let db = gbd_o * dvbd + gbs_o * dvbs;
                    let tol =
                        |i: f64, d: f64| self.o.reltol * (i + d).abs().max(i.abs()) + self.o.abstol;
                    if dch.abs() > tol(id_o, dch) || db.abs() > tol(cb_o, db) {
                        limited = true;
                    }
                    let mut vgd = vgs - vds;
                    if vds_o >= 0.0 {
                        vgs = fetlim(vgs, vgs_o, von);
                        vds = limvds(vgs - vgd, vds_o);
                    } else {
                        vgd = fetlim(vgd, vgs_o - vds_o, von);
                        vds = -limvds(-(vgs - vgd), -vds_o);
                        vgs = vgd + vds;
                    }
                    if vds >= 0.0 {
                        let (vl, changed) = pnjlim(vbs, vbs_o, VT, vcrit);
                        vbs = vl;
                        limited |= changed;
                    } else {
                        let (vl, changed) = pnjlim(vbs - vds, vbs_o - vds_o, VT, vcrit);
                        vbs = vl + vds;
                        limited |= changed;
                    }
                }
                let vbd = vbs - vds;
                let vgd = vgs - vds;
                let leff = m.l - 2.0 * model.ld;
                let beta = model.kp * m.w / leff * m.m;
                let (cbs, gbs) = bulk_junction(is, vbs, ctx.gmin);
                let (cbd, gbd) = bulk_junction(is, vbd, ctx.gmin);
                let forward = vds >= 0.0;
                let (id, gm, gds, gmbs, von) = if forward {
                    mos1_channel(model, beta, vgs, vds, vbs)
                } else {
                    mos1_channel(model, beta, vgd, -vds, vbd)
                };
                *lim = [
                    vgs,
                    vds,
                    vbs,
                    von,
                    id,
                    gm,
                    gds,
                    gmbs,
                    gbd,
                    gbs,
                    cbs + cbd,
                    if forward { 1.0 } else { -1.0 },
                ];
                // In reverse mode the source and drain swap roles.
                let (de, se, vgse, vdse, vbse) =
                    if forward { (m.d, m.s, vgs, vds, vbs) } else { (m.s, m.d, vgd, -vds, vbd) };
                current(
                    s,
                    l,
                    de,
                    se,
                    p * id,
                    &[(m.g, se, gm, p * vgse), (de, se, gds, p * vdse), (m.b, se, gmbs, p * vbse)],
                );
                current(s, l, m.b, m.d, p * cbd, &[(m.b, m.d, gbd, p * vbd)]);
                current(s, l, m.b, m.s, p * cbs, &[(m.b, m.s, gbs, p * vbs)]);

                // Gate capacitances: Meyer's intrinsic model plus overlap, integrated
                // incrementally with the value averaged over the step, as SPICE does.
                let vgb = vgs - vbs;
                let vdsat = (if forward { vgs } else { vgd } - von).max(0.0);
                let cox = model.cox() * leff * m.w * m.m;
                let (hgs, hgd, hgb) = if forward {
                    crate::device::qmeyer(vgs, vgd, vgb, von, vdsat, model.phi, cox)
                } else {
                    let (gd, gs, gb) =
                        crate::device::qmeyer(vgd, vgs, vgb, von, vdsat, model.phi, cox);
                    (gs, gd, gb)
                };
                let gates = [
                    (m.s, hgs, model.cgso * m.w * m.m, p * vgs),
                    (m.d, hgd, model.cgdo * m.w * m.m, p * vgd),
                    (m.b, hgb, model.cgbo * leff * m.m, p * vgb),
                ];
                for (k, (other, half, overlap, vg)) in gates.into_iter().enumerate() {
                    let slot = dev.slot + k;
                    if ctx.ag0 == 0.0 {
                        let c = 2.0 * half + overlap;
                        now[slot] = Slot { q: c * vg, i: 0.0, v: vg, c: half };
                        continue;
                    }
                    let h = ctx.hist[slot];
                    let c = half + h.c + overlap;
                    let q = h.q + c * (vg - h.v);
                    let (i, geq) = Self::integrate(ctx, slot, q, c);
                    now[slot] = Slot { q, i, v: vg, c: half };
                    if c != 0.0 {
                        current(s, l, m.g, other, i, &[(m.g, other, geq, vg)]);
                    }
                }
                // Bulk junction depletion charge.
                let area = |given: f64, a: f64, perim: f64, v: f64| {
                    if given > 0.0 {
                        depletion(given * m.m, model.pb, model.mj, model.fc, v)
                    } else {
                        let (q1, c1) =
                            depletion(model.cj * a * m.m, model.pb, model.mj, model.fc, v);
                        let (q2, c2) =
                            depletion(model.cjsw * perim * m.m, model.pb, model.mjsw, model.fc, v);
                        (q1 + q2, c1 + c2)
                    }
                };
                let junctions = [
                    (m.d, area(model.cbd, m.ad, m.pd, vbd), vbd),
                    (m.s, area(model.cbs, m.as_, m.ps, vbs), vbs),
                ];
                for (k, (node, (qn, c), vj)) in junctions.into_iter().enumerate() {
                    let slot = dev.slot + 3 + k;
                    let q = p * qn;
                    let (i, geq) = Self::integrate(ctx, slot, q, c);
                    now[slot] = Slot { q, i, ..Slot::default() };
                    if ctx.ag0 > 0.0 && c != 0.0 {
                        current(s, l, m.b, node, i, &[(m.b, node, geq, p * vj)]);
                    }
                }
            }
        }
        limited
    }

    fn load(
        &self,
        x: &[f64],
        ctx: &Ctx,
        lim: &mut Limits,
        now: &mut [Slot],
        sys: &mut dyn Stamp,
    ) -> bool {
        let mut limited = false;
        for (di, state) in lim.iter_mut().enumerate() {
            limited |= self.load_device(di, x, ctx, state, now, sys);
        }
        limited
    }

    fn converged(&self, old: &[f64], new: &[f64]) -> Result<(), usize> {
        let nodes = self.c.nodes.len() - 1;
        let mut worst = (0.0, 0);
        for (k, (a, b)) in old.iter().zip(new).enumerate() {
            let tol = self.o.reltol * a.abs().max(b.abs())
                + if k < nodes { self.o.vntol } else { self.o.abstol };
            let ratio = (a - b).abs() / tol;
            if ratio > worst.0 {
                worst = (ratio, k);
            }
        }
        if worst.0 <= 1.0 { Ok(()) } else { Err(worst.1) }
    }

    fn newton(
        &self,
        x: &mut [f64],
        ctx: &mut Ctx,
        lim: &mut Limits,
        now: &mut [Slot],
        max_iter: usize,
        gshunt: f64,
    ) -> Result<usize, Fail> {
        let mut sys = self.system();
        let nodes = self.c.nodes.len() - 1;
        let mut worst = 0;
        for iter in 0..max_iter {
            let limited = loop {
                sys.clear();
                let limited = self.load(x, ctx, lim, now, &mut sys);
                for k in 0..nodes {
                    sys.a(k, k, gshunt + self.o.leakage);
                }
                if sys.missing.is_empty() {
                    break limited;
                }
                self.grow(&sys.missing);
                sys = self.system();
            };
            // A singular matrix after the first iteration is the iteration running away,
            // not the circuit: report it as such so the caller tries a gentler strategy.
            let new = match self.solve(&sys) {
                Ok(v) => v,
                Err(col) if iter == 0 => return Err(Fail::Singular(col)),
                Err(_) => return Err(Fail::NoConvergence(worst)),
            };
            if new.iter().any(|v| !v.is_finite()) {
                return Err(Fail::NoConvergence(worst));
            }
            let check = self.converged(x, &new);
            let first = ctx.init;
            x.copy_from_slice(&new);
            ctx.init = false;
            match check {
                Ok(()) if iter > 0 && !limited && !first => return Ok(iter + 1),
                Err(row) => worst = row,
                _ => {}
            }
        }
        Err(Fail::NoConvergence(worst))
    }

    /// Whether the topology alone makes the equations singular.
    fn structurally_singular(&self) -> bool {
        self.c.topology_problems().iter().any(|p| match p {
            Problem::SourceLoop { .. } => true,
            Problem::Floating { .. } => self.o.leakage == 0.0,
        })
    }

    fn fail(&self, f: Fail, what: &str) -> SimError {
        match f {
            Fail::Singular(row) => {
                let name = self.row_name(row);
                let mut message = format!("{what}: the circuit equations are singular at {name}");
                let mut devices = Vec::new();
                for p in self.c.topology_problems() {
                    match p {
                        Problem::SourceLoop { device } => {
                            message = format!(
                                "{what}: {device} closes a loop of voltage sources and \
                                 inductors, so nothing limits the current around it"
                            );
                            devices.push(device);
                        }
                        Problem::Floating { nodes } if self.o.leakage == 0.0 => {
                            message =
                                format!("{what}: no DC path to ground from {}", nodes.join(", "));
                        }
                        Problem::Floating { .. } => {}
                    }
                }
                SimError { message, nodes: vec![name], devices }
            }
            Fail::NoConvergence(row) => {
                let name = self.row_name(row);
                SimError {
                    message: format!(
                        "{what}: no convergence; the solution kept changing at {name}"
                    ),
                    nodes: vec![name],
                    devices: Vec::new(),
                }
            }
        }
    }

    /// Finds an operating point: plain Newton, then gmin stepping, then source stepping.
    fn operating_point(
        &self,
        time: Option<f64>,
        tstep: f64,
        tstop: f64,
        guess: Option<&[f64]>,
    ) -> Result<(Vec<f64>, Limits, Vec<Slot>), SimError> {
        let hist = vec![Slot::default(); self.c.slots];
        let ctx = |init| Ctx {
            time,
            tstep,
            tstop,
            scale: 1.0,
            ag0: 0.0,
            trap: false,
            hist: &hist,
            init,
            gmin: self.o.gmin,
        };
        let fresh = || {
            (
                vec![0.0; self.n],
                vec![[0.0; 12]; self.c.devices.len()],
                vec![Slot::default(); self.c.slots],
            )
        };

        if let Some(g) = guess {
            let (mut x, _, mut now) = fresh();
            x.copy_from_slice(g);
            let mut lim = self.limits_at(&x);
            if self.newton(&mut x, &mut ctx(false), &mut lim, &mut now, self.o.itl1, 0.0).is_ok() {
                return Ok((x, lim, now));
            }
        }

        let (mut x, mut lim, mut now) = fresh();
        // A structurally singular circuit (a floating node, a loop of sources) can't be
        // helped by stepping; report it at once.
        let last = match self.newton(&mut x, &mut ctx(true), &mut lim, &mut now, self.o.itl1, 0.0) {
            Ok(_) => return Ok((x, lim, now)),
            Err(e @ Fail::Singular(_)) if self.structurally_singular() => {
                return Err(self.fail(e, "operating point"));
            }
            Err(e) => e,
        };

        // Dynamic gmin stepping (as ngspice does): shunt every node with a conductance,
        // start heavily damped and relax, faster when steps converge easily and more
        // gently after a failure.
        let (mut x, mut lim, mut now) = fresh();
        let itl = 50;
        let target = self.o.gmin;
        let mut factor: f64 = 10.0;
        let mut old_g = 1e-2;
        let mut g = old_g / factor;
        let mut init = true;
        let mut saved = (x.clone(), lim.clone(), now.clone());
        let stepped = loop {
            match self.newton(&mut x, &mut ctx(init), &mut lim, &mut now, itl, g) {
                Ok(iters) => {
                    init = false;
                    if g <= target {
                        break true;
                    }
                    saved = (x.clone(), lim.clone(), now.clone());
                    if iters <= itl / 4 {
                        factor = (factor * factor.sqrt()).min(10.0);
                    }
                    if iters > 3 * itl / 4 {
                        factor = factor.sqrt();
                    }
                    old_g = g;
                    if g < factor * target {
                        g = target;
                    } else {
                        g /= factor;
                    }
                }
                Err(_) => {
                    if factor < 1.00005 {
                        break false;
                    }
                    factor = factor.sqrt().sqrt();
                    g = old_g / factor;
                    (x, lim, now) = saved.clone();
                }
            }
        };
        if stepped
            && self.newton(&mut x, &mut ctx(false), &mut lim, &mut now, self.o.itl1, 0.0).is_ok()
        {
            return Ok((x, lim, now));
        }

        // Source stepping: ramp every source up from zero.
        let (mut x, mut lim, mut now) = fresh();
        let mut scale: f64 = 0.0;
        let mut step: f64 = 0.1;
        let mut init = true;
        while scale < 1.0 {
            let next = (scale + step).min(1.0);
            let saved = (x.clone(), lim.clone());
            let mut c = ctx(init);
            c.scale = next;
            match self.newton(&mut x, &mut c, &mut lim, &mut now, self.o.itl1, 0.0) {
                Ok(_) => {
                    scale = next;
                    step = (step * 2.0).min(0.25);
                    init = false;
                }
                Err(_) => {
                    (x, lim) = saved;
                    step /= 4.0;
                    if step < 1e-4 {
                        return Err(self.fail(last, "operating point"));
                    }
                }
            }
        }
        Ok((x, lim, now))
    }

    /// The limiting memory matching solution `x`, to continue Newton from it.
    fn limits_at(&self, x: &[f64]) -> Limits {
        let v = |k: usize| if k == 0 { 0.0 } else { x[k - 1] };
        self.c
            .devices
            .iter()
            .map(|dev| match &dev.element {
                Element::Diode(d) => {
                    let vd = v(d.junction) - v(d.cathode);
                    let (id, gd) = d.iv(vd, self.o.gmin);
                    let mut l = [0.0; 12];
                    l[..3].copy_from_slice(&[vd, id, gd]);
                    l
                }
                Element::Mosfet(m) => {
                    let p = m.model.polarity;
                    let (vgs, vds, vbs) =
                        (p * (v(m.g) - v(m.s)), p * (v(m.d) - v(m.s)), p * (v(m.b) - v(m.s)));
                    let beta = m.model.kp * m.w / (m.l - 2.0 * m.model.ld) * m.m;
                    let forward = vds >= 0.0;
                    let (id, gm, gds, gmbs, von) = if forward {
                        mos1_channel(&m.model, beta, vgs, vds, vbs)
                    } else {
                        mos1_channel(&m.model, beta, vgs - vds, -vds, vbs - vds)
                    };
                    let is = m.model.is * m.m;
                    let (cbs, gbs) = bulk_junction(is, vbs, self.o.gmin);
                    let (cbd, gbd) = bulk_junction(is, vbs - vds, self.o.gmin);
                    let mode = if forward { 1.0 } else { -1.0 };
                    [vgs, vds, vbs, von, id, gm, gds, gmbs, gbd, gbs, cbs + cbd, mode]
                }
                Element::Bjt(q) => {
                    let p = q.model.polarity;
                    let (vbe, vbc) = (p * (v(q.bi) - v(q.ei)), p * (v(q.bi) - v(q.ci)));
                    let st = q.eval(vbe, vbc, self.o.gmin);
                    [vbe, vbc, st.ic, st.ib, st.gm, st.go, st.gpi, st.gmu, 0.0, 0.0, 0.0, 0.0]
                }
                _ => [0.0; 12],
            })
            .collect()
    }

    fn device_ops(&self, x: &[f64], lim: &Limits) -> Vec<DeviceOp> {
        (0..self.c.devices.len()).map(|di| self.device_op(di, x, &lim[di], None)).collect()
    }

    /// What device `di` is doing at solution `x`: the static currents into its terminals
    /// (charging currents excluded, since they dissipate nothing) and the power it
    /// absorbs. `time` sets independent sources during a transient run.
    fn device_op(&self, di: usize, x: &[f64], lim: &[f64; 12], time: Option<f64>) -> DeviceOp {
        let hist = vec![Slot::default(); self.c.slots];
        let ctx = Ctx {
            time,
            tstep: self.tstep.get(),
            tstop: self.tstop.get(),
            scale: 1.0,
            ag0: 0.0,
            trap: false,
            hist: &hist,
            init: false,
            gmin: self.o.gmin,
        };
        let v = |k: usize| if k == 0 { 0.0 } else { x[k - 1] };
        let dev = &self.c.devices[di];
        let mut now = vec![Slot::default(); self.c.slots];
        let mut col = Collect::default();
        let mut l = *lim;
        self.load_device(di, x, &ctx, &mut l, &mut now, &mut col);
        let terminals = dev.terminals();
        let mut currents: Vec<f64> = terminals
            .iter()
            .map(|&t| {
                let Some(row) = self.l.node(t) else { return f64::NAN };
                let ax: f64 = col.a.iter().filter(|e| e.0 == row).map(|e| e.2 * x[e.1]).sum();
                let b: f64 = col.b.iter().filter(|e| e.0 == row).map(|e| e.1).sum();
                ax - b
            })
            .collect();
        // A grounded terminal carries whatever the others don't (KCL).
        let known: f64 = currents.iter().filter(|c| c.is_finite()).sum();
        let grounded = currents.iter().filter(|c| c.is_nan()).count();
        for c in currents.iter_mut().filter(|c| c.is_nan()) {
            *c = -known / grounded as f64;
        }
        let power = terminals.iter().zip(&currents).map(|(&t, i)| v(t) * i).sum();
        DeviceOp { name: dev.name.clone(), currents, power }
    }
}

/// The DC operating point, with capacitors open and inductors shorted.
pub fn op(c: &Circuit, o: &Options) -> Result<OpResult, SimError> {
    let e = Engine::new(c, o);
    let (x, lim, _) = e.operating_point(None, 1.0, 1.0, None)?;
    let nodes = c.nodes.len();
    let mut voltages = vec![0.0];
    voltages.extend_from_slice(&x[..nodes - 1]);
    Ok(OpResult {
        voltages,
        branch_currents: x[nodes - 1..].to_vec(),
        devices: e.device_ops(&x, &lim),
    })
}

/// Sweeps a voltage or current source's DC value, solving an operating point at each.
pub fn dc_sweep(
    c: &Circuit,
    source: &str,
    values: &[f64],
    o: &Options,
) -> Result<Vec<OpResult>, SimError> {
    let k = c.device(source).ok_or_else(|| SimError {
        message: format!("no source `{source}`"),
        nodes: Vec::new(),
        devices: Vec::new(),
    })?;
    let mut circuit = c.clone();
    let mut guess: Option<Vec<f64>> = None;
    let mut out = Vec::with_capacity(values.len());
    for (n, &value) in values.iter().enumerate() {
        o.progress(n as f64 / values.len() as f64)?;
        match &mut circuit.devices[k].element {
            Element::Vsource { source, .. } | Element::Isource { source, .. } => {
                source.spec.dc = Some(value)
            }
            _ => {
                return Err(SimError {
                    message: format!("{source} is not an independent source"),
                    nodes: Vec::new(),
                    devices: vec![source.to_owned()],
                });
            }
        }
        let e = Engine::new(&circuit, o);
        let (x, lim, _) = e.operating_point(None, 1.0, 1.0, guess.as_deref())?;
        let nodes = circuit.nodes.len();
        let mut voltages = vec![0.0];
        voltages.extend_from_slice(&x[..nodes - 1]);
        out.push(OpResult {
            voltages,
            branch_currents: x[nodes - 1..].to_vec(),
            devices: e.device_ops(&x, &lim),
        });
        guess = Some(x);
    }
    Ok(out)
}

/// Watches a transient run, and may change the circuit as it goes: the physics
/// engine uses this to heat parts and to swap a destroyed part for its failure mode.
pub trait Observer {
    /// Whether accepting `step` would change the circuit, asked without committing
    /// anything, so the solver can shorten the step to place the change accurately.
    fn fails(&self, _step: &Step<'_>) -> bool {
        false
    }

    /// Called after the operating point and after every accepted time point. Returns
    /// devices to replace: each `(index, element)` takes effect from this time on.
    fn step(&mut self, step: &Step<'_>) -> Vec<(usize, Element)>;
}

/// An observer that only watches.
#[derive(Debug, Default)]
pub struct NoObserver;

impl Observer for NoObserver {
    fn step(&mut self, _: &Step<'_>) -> Vec<(usize, Element)> {
        Vec::new()
    }
}

/// The circuit at one accepted time point, as an [`Observer`] sees it.
pub struct Step<'a> {
    engine: &'a Engine<'a>,
    x: &'a [f64],
    lim: &'a Limits,
    /// The time of this point.
    pub time: f64,
    /// The step that led here; zero for the operating point.
    pub dt: f64,
}

impl std::fmt::Debug for Step<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step").field("time", &self.time).field("dt", &self.dt).finish()
    }
}

impl Step<'_> {
    pub fn circuit(&self) -> &Circuit {
        self.engine.c
    }

    pub fn voltage(&self, node: usize) -> f64 {
        if node == 0 { 0.0 } else { self.x[node - 1] }
    }

    /// Device `d`'s static terminal currents and dissipated power at this point.
    pub fn device(&self, d: usize) -> DeviceOp {
        self.engine.device_op(d, self.x, &self.lim[d], Some(self.time))
    }
}

/// A transient analysis from 0 to `tstop`. `tstep` is the output resolution you
/// want: the step never exceeds it (or `tstop / 50`), and it shrinks as needed to
/// keep truncation error within tolerance and to land on every source corner.
pub fn tran(c: &Circuit, tstop: f64, tstep: f64, o: &Options) -> Result<TranResult, SimError> {
    tran_observed(c, tstop, tstep, o, &mut NoObserver).map(|(r, _)| r)
}

/// A transient analysis watched by `observer`, which may replace devices as the run
/// goes. Returns the results and the circuit as it ended up.
pub fn tran_observed(
    c: &Circuit,
    tstop: f64,
    tstep: f64,
    o: &Options,
    observer: &mut dyn Observer,
) -> Result<(TranResult, Circuit), SimError> {
    if !(tstop > 0.0 && tstep > 0.0) {
        return Err(SimError {
            message: "the stop time and step must be positive".into(),
            nodes: Vec::new(),
            devices: Vec::new(),
        });
    }
    let mut circuit = c.clone();
    let mut e = Engine::new(&circuit, o);
    e.tstep.set(tstep);
    e.tstop.set(tstop);
    let (mut x, mut lim, now0) = if o.power_on {
        // Everything off: zero voltages, no stored charge.
        let x = vec![0.0; e.n];
        let lim = e.limits_at(&x);
        (x, lim, vec![Slot::default(); circuit.slots])
    } else {
        e.operating_point(Some(0.0), tstep, tstop, None)?
    };
    let nodes = c.nodes.len();
    let hmax = tstep.min(tstop / 50.0);
    let hmin = hmax * 1e-9;

    // Corners of the source waveforms, where the step must land.
    let mut bps: Vec<f64> = c
        .devices
        .iter()
        .flat_map(|d| match &d.element {
            Element::Vsource { source, .. } | Element::Isource { source, .. } => {
                source.breakpoints(tstep, tstop)
            }
            _ => Vec::new(),
        })
        .filter(|&t| t > 0.0 && t < tstop)
        .collect();
    bps.push(tstop);
    bps.sort_by(f64::total_cmp);
    bps.dedup_by(|a, b| (*a - *b).abs() <= hmin);

    let charges = c.charge_slots();
    let mut hist = now0;
    let mut qhist: Vec<[f64; 3]> = hist.iter().map(|s| [s.q, 0.0, 0.0]).collect();
    let mut thist = [0.0; 3];
    let mut valid = 1;

    let mut result = TranResult { time: vec![0.0], values: vec![x.clone()], nodes, rejected: 0 };
    // The operating point: parts may already be beyond saving.
    let changes = observer.step(&Step { engine: &e, x: &x, lim: &lim, time: 0.0, dt: 0.0 });
    if !changes.is_empty() {
        for (d, element) in changes {
            circuit.devices[d].element = element;
        }
        e = Engine::new(&circuit, o);
        e.tstep.set(tstep);
        e.tstop.set(tstop);
        lim = e.limits_at(&x);
    }

    let mut t = 0.0;
    let mut h = (tstop / 100.0).min(tstep) / 10.0;
    let mut order = 1;
    let mut bi = 0;
    while t < tstop - hmin {
        while bi < bps.len() && bps[bi] <= t + hmin {
            bi += 1;
        }
        let next_bp = bps.get(bi).copied().unwrap_or(tstop);
        let mut step = h.min(hmax);
        let mut hit = false;
        if t + step > next_bp - 0.1 * step {
            step = next_bp - t;
            hit = true;
        }
        let ag0 = if order == 1 { 1.0 / step } else { 2.0 / step };
        let mut ctx = Ctx {
            time: Some(t + step),
            tstep,
            tstop,
            scale: 1.0,
            ag0,
            trap: order == 2,
            hist: &hist,
            init: false,
            gmin: o.gmin,
        };
        let mut x_new = x.clone();
        let mut lim_new = lim.clone();
        let mut now = hist.clone();
        if e.newton(&mut x_new, &mut ctx, &mut lim_new, &mut now, o.itl4, 0.0).is_err() {
            result.rejected += 1;
            h = step / 8.0;
            order = 1;
            if h < hmin {
                return Err(SimError {
                    message: format!(
                        "transient: the time step became too small at t = {:.4e} s; \
                         the circuit changes faster than it can be followed",
                        t
                    ),
                    nodes: Vec::new(),
                    devices: Vec::new(),
                });
            }
            continue;
        }

        // A switch changed state: narrow the step until the change is pinned down,
        // then treat it as a corner.
        let switched =
            now.iter().zip(&hist).enumerate().any(|(k, (a, b))| !charges[k] && a.q != b.q);
        if switched && step > hmax * 0.01 {
            result.rejected += 1;
            h = (step / 4.0).max(hmin);
            order = 1;
            continue;
        }

        // Truncation error, from divided differences of each charge's history.
        let t1 = t + step;
        let mut ratio = f64::INFINITY;
        if valid > order {
            for (k, s) in now.iter().enumerate().filter(|(k, _)| charges[*k]) {
                let q = [s.q, qhist[k][0], qhist[k][1], qhist[k][2]];
                let ts = [t1, thist[0], thist[1], thist[2]];
                let dd1 = |i: usize| (q[i] - q[i + 1]) / (ts[i] - ts[i + 1]);
                let dd2 = |i: usize| (dd1(i) - dd1(i + 1)) / (ts[i] - ts[i + 2]);
                let lte = if order == 1 {
                    step * step * dd2(0).abs()
                } else {
                    let dd3 = (dd2(0) - dd2(1)) / (ts[0] - ts[3]);
                    step * step * step / 2.0 * dd3.abs()
                };
                if lte > 0.0 && lte.is_finite() {
                    let tol = o.trtol
                        * (o.reltol * s.q.abs().max(q[1].abs()) + o.chgtol)
                            .max((o.reltol * s.i.abs().max(hist[k].i.abs()) + o.abstol) * step);
                    ratio = ratio.min(tol / lte);
                }
            }
        }
        let h_new = if ratio.is_finite() {
            step * ratio.powf(1.0 / (order as f64 + 1.0))
        } else {
            2.0 * step
        };
        if h_new < 0.9 * step && step > 2.0 * hmin {
            result.rejected += 1;
            h = h_new.max(step / 8.0).max(hmin);
            continue;
        }

        // The observer would change the circuit here: narrow the step to place the
        // change within a hundredth of the largest step, as for a switch.
        if step > hmax * 0.01
            && observer.fails(&Step { engine: &e, x: &x_new, lim: &lim_new, time: t1, dt: step })
        {
            result.rejected += 1;
            h = (step / 4.0).max(hmin);
            order = 1;
            continue;
        }

        // Accept.
        o.progress(t1 / tstop)?;
        t = t1;
        x = x_new;
        lim = lim_new;
        for (k, s) in now.iter().enumerate() {
            qhist[k] = [s.q, qhist[k][0], qhist[k][1]];
        }
        thist = [t, thist[0], thist[1]];
        valid = (valid + 1).min(3);
        hist = now;
        result.time.push(t);
        result.values.push(x.clone());
        if result.time.len() > o.max_points {
            return Err(SimError {
                message: format!("transient: more than {} time points", o.max_points),
                nodes: Vec::new(),
                devices: Vec::new(),
            });
        }

        h = h_new.min(2.0 * step);
        if switched {
            order = 1;
            h = h.min(step.max(hmax * 0.01));
        } else if hit {
            // Restart gently after a corner: first order, small step.
            order = 1;
            let after = bps.get(bi + 1).copied().unwrap_or(tstop);
            h = h.min(0.1 * step.min(after - t)).min(hmax);
        } else {
            order = 2;
        }
        h = h.clamp(hmin, hmax);

        // The observer may destroy parts: rebuild around their replacements and restart
        // gently, as after a corner.
        let changes = observer.step(&Step { engine: &e, x: &x, lim: &lim, time: t, dt: step });
        if !changes.is_empty() {
            for (d, element) in changes {
                circuit.devices[d].element = element;
            }
            e = Engine::new(&circuit, o);
            e.tstep.set(tstep);
            e.tstop.set(tstop);
            lim = e.limits_at(&x);
            order = 1;
            h = h.min(hmax * 0.01).max(hmin);
        }
    }
    Ok((result, circuit))
}

/// A complex number, for AC analysis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Complex { re, im }
    }

    pub fn polar(mag: f64, degrees: f64) -> Self {
        let a = degrees.to_radians();
        Complex::new(mag * a.cos(), mag * a.sin())
    }

    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Magnitude in decibels (20 log₁₀).
    pub fn db(self) -> f64 {
        20.0 * self.abs().log10()
    }

    /// Phase in degrees.
    pub fn phase(self) -> f64 {
        self.im.atan2(self.re).to_degrees()
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, o: Complex) -> Complex {
        Complex::new(self.re + o.re, self.im + o.im)
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, o: Complex) -> Complex {
        Complex::new(self.re - o.re, self.im - o.im)
    }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, o: Complex) -> Complex {
        Complex::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}

impl std::ops::Div for Complex {
    type Output = Complex;
    fn div(self, o: Complex) -> Complex {
        let d = o.re * o.re + o.im * o.im;
        Complex::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
}

impl std::ops::Neg for Complex {
    type Output = Complex;
    fn neg(self) -> Complex {
        Complex::new(-self.re, -self.im)
    }
}

impl Scalar for Complex {
    fn magnitude(self) -> f64 {
        self.abs()
    }
}

/// How frequencies are spaced in an AC sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    /// Points per decade.
    Decade(usize),
    /// Points per octave.
    Octave(usize),
    /// Points in total, evenly spaced.
    Linear(usize),
}

impl Spacing {
    pub fn frequencies(self, fstart: f64, fstop: f64) -> Vec<f64> {
        match self {
            Spacing::Linear(n) => {
                let n = n.max(2);
                (0..n).map(|k| fstart + (fstop - fstart) * k as f64 / (n - 1) as f64).collect()
            }
            Spacing::Decade(n) | Spacing::Octave(n) => {
                let base: f64 = if matches!(self, Spacing::Decade(_)) { 10.0 } else { 2.0 };
                let ratio = base.powf(1.0 / n.max(1) as f64);
                let mut out = Vec::new();
                let mut f = fstart;
                while f <= fstop * (1.0 + 1e-9) && out.len() < 100_000 {
                    out.push(f);
                    f *= ratio;
                }
                out
            }
        }
    }
}

/// Small-signal response at each frequency: every unknown as a complex amplitude.
#[derive(Debug, Clone, PartialEq)]
pub struct AcResult {
    pub frequency: Vec<f64>,
    pub values: Vec<Vec<Complex>>,
}

impl AcResult {
    pub fn voltage(&self, c: &Circuit, node: &str) -> Option<Vec<Complex>> {
        let k = c.node(node)?;
        Some(
            self.values
                .iter()
                .map(|row| if k == 0 { Complex::default() } else { row[k - 1] })
                .collect(),
        )
    }
}

/// Small-signal AC analysis about the operating point. Sources with an `AC` value
/// drive the circuit; everything else is linearised where it sits.
pub fn ac(
    c: &Circuit,
    spacing: Spacing,
    fstart: f64,
    fstop: f64,
    o: &Options,
) -> Result<AcResult, SimError> {
    if !(fstart > 0.0 && fstop >= fstart) {
        return Err(SimError {
            message: "the start frequency must be positive and no higher than the stop".into(),
            nodes: Vec::new(),
            devices: Vec::new(),
        });
    }
    let e = Engine::new(c, o);
    let (x, lim, now) = e.operating_point(None, 1.0, 1.0, None)?;
    // The conductance matrix G, and G + C from the same devices with a unit integration
    // coefficient: their difference is the capacitance matrix.
    let stamp = |ag0: f64| -> SparseSystem {
        let ctx = Ctx {
            time: None,
            tstep: 1.0,
            tstop: 1.0,
            scale: 1.0,
            ag0,
            trap: false,
            hist: &now,
            init: false,
            gmin: o.gmin,
        };
        loop {
            let mut sys = e.system();
            let mut l = lim.clone();
            let mut scratch = now.clone();
            e.load(&x, &ctx, &mut l, &mut scratch, &mut sys);
            for k in 0..c.nodes.len() - 1 {
                sys.a(k, k, o.leakage);
            }
            if sys.missing.is_empty() {
                return sys;
            }
            e.grow(&sys.missing);
        }
    };
    // Both in one structure: the second stamp may have grown it.
    let mut gc = stamp(1.0);
    let mut g = stamp(0.0);
    if g.pattern != gc.pattern {
        gc = stamp(1.0);
        g = stamp(0.0);
    }
    let pattern = g.pattern.clone();
    let order = minimum_degree(&pattern);

    // Excitation from the sources' AC values.
    let mut rhs = vec![Complex::default(); e.n];
    for dev in &c.devices {
        match &dev.element {
            Element::Vsource { branch, source, .. } => {
                if let Some((mag, ph)) = source.spec.ac {
                    rhs[e.l.branch(*branch)] = Complex::polar(mag, ph);
                }
            }
            Element::Isource { p, n, source } => {
                if let Some((mag, ph)) = source.spec.ac {
                    let i = Complex::polar(mag, ph);
                    if let Some(r) = e.l.node(*p) {
                        rhs[r] = rhs[r] - i;
                    }
                    if let Some(r) = e.l.node(*n) {
                        rhs[r] = rhs[r] + i;
                    }
                }
            }
            _ => {}
        }
    }

    let frequency = spacing.frequencies(fstart, fstop);
    let mut values = Vec::with_capacity(frequency.len());
    let mut lu: Option<Lu<Complex>> = None;
    for (k, &f) in frequency.iter().enumerate() {
        o.progress(k as f64 / frequency.len() as f64)?;
        let w = std::f64::consts::TAU * f;
        let a: Vec<Complex> = g
            .values
            .iter()
            .zip(&gc.values)
            .map(|(&g, &gc)| Complex::new(g, w * (gc - g)))
            .collect();
        let reused = lu.as_mut().is_some_and(|lu| lu.refactor(&pattern, &a));
        if !reused {
            lu = Some(Lu::factor(&pattern, &a, &order, PIVOT_THRESHOLD).map_err(|row| {
                SimError {
                    message: format!(
                        "AC analysis: the equations are singular at {}",
                        e.row_name(row)
                    ),
                    nodes: vec![e.row_name(row)],
                    devices: Vec::new(),
                }
            })?);
        }
        let mut b = rhs.clone();
        lu.as_ref().expect("factored").solve(&mut b);
        values.push(b);
    }
    Ok(AcResult { frequency, values })
}
