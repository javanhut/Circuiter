//! Device models and how each one is stamped into the circuit equations.
//!
//! The diode and MOSFET Level 1 equations, junction voltage limiting (`pnjlim`,
//! `fetlim`, `limvds`) and initial junction guesses follow Berkeley SPICE3f5
//! (Copyright 1985-1993 The Regents of the University of California, BSD licensed), as
//! ngspice does, so results agree with it.

use crate::parse::{ModelCard, SourceSpec, Waveform};

/// Thermal voltage kT/q at the nominal temperature, 27 °C.
pub const VT: f64 = 8.617_333_262e-5 * 300.15;

/// Where a device's equations go: the system matrix and right-hand side, by node or
/// branch row. Ground (node 0) is dropped.
pub trait Stamp {
    fn a(&mut self, row: usize, col: usize, v: f64);
    fn b(&mut self, row: usize, v: f64);
}

/// Unknowns: node `k` (k ≥ 1) is row `k - 1`; branch `j` follows the nodes.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub nodes: usize,
}

impl Layout {
    pub fn node(&self, k: usize) -> Option<usize> {
        k.checked_sub(1)
    }

    pub fn branch(&self, j: usize) -> usize {
        self.nodes - 1 + j
    }

    pub fn size(&self, branches: usize) -> usize {
        self.nodes - 1 + branches
    }
}

fn add_a(s: &mut dyn Stamp, l: Layout, r: usize, c: usize, v: f64) {
    if let (Some(r), Some(c)) = (l.node(r), l.node(c)) {
        s.a(r, c, v);
    }
}

fn add_b(s: &mut dyn Stamp, l: Layout, r: usize, v: f64) {
    if let Some(r) = l.node(r) {
        s.b(r, v);
    }
}

/// A conductance `g` between nodes `p` and `n`.
pub fn conductance(s: &mut dyn Stamp, l: Layout, p: usize, n: usize, g: f64) {
    add_a(s, l, p, p, g);
    add_a(s, l, n, n, g);
    add_a(s, l, p, n, -g);
    add_a(s, l, n, p, -g);
}

/// A current flowing through a device from node `from` to node `to`, linearised about
/// the controlling voltages: `i = i0 + Σ g (v(cp) - v(cn) - v0)`.
pub fn current(
    s: &mut dyn Stamp,
    l: Layout,
    from: usize,
    to: usize,
    i0: f64,
    controls: &[(usize, usize, f64, f64)],
) {
    let mut ieq = i0;
    for &(cp, cn, g, v0) in controls {
        add_a(s, l, from, cp, g);
        add_a(s, l, from, cn, -g);
        add_a(s, l, to, cp, -g);
        add_a(s, l, to, cn, g);
        ieq -= g * v0;
    }
    add_b(s, l, from, -ieq);
    add_b(s, l, to, ieq);
}

/// An independent source's value over time.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub spec: SourceSpec,
}

impl Source {
    /// The DC value: the `DC` value if given, else the transient function at time zero.
    pub fn dc(&self) -> f64 {
        match (&self.spec.dc, &self.spec.tran) {
            (Some(v), _) => *v,
            (None, Some(w)) => waveform_at(w, 0.0, 1.0, 1.0),
            (None, None) => 0.0,
        }
    }

    /// The value at time `t` of a transient analysis with print step `tstep` ending at
    /// `tstop`, which set the defaults of unspecified waveform times.
    pub fn at(&self, t: f64, tstep: f64, tstop: f64) -> f64 {
        match &self.spec.tran {
            Some(w) => waveform_at(w, t, tstep, tstop),
            None => self.dc(),
        }
    }

    /// Times where the waveform has corners, which the time step must land on.
    pub fn breakpoints(&self, tstep: f64, tstop: f64) -> Vec<f64> {
        let mut out = Vec::new();
        match &self.spec.tran {
            Some(Waveform::Pulse { td, tr, tf, pw, per, .. }) => {
                let (tr, tf) = (tr.unwrap_or(tstep), tf.unwrap_or(tstep));
                let (pw, per) = (pw.unwrap_or(tstop), per.unwrap_or(tstop));
                let mut start = *td;
                let mut count = 0;
                while start < tstop && count < 1_000_000 {
                    for t in [start, start + tr, start + tr + pw, start + tr + pw + tf] {
                        if t <= tstop {
                            out.push(t);
                        }
                    }
                    if per <= 0.0 {
                        break;
                    }
                    start += per;
                    count += 1;
                }
            }
            Some(Waveform::Sin { td, .. }) if *td > 0.0 => out.push(*td),
            Some(Waveform::Pwl(points)) => out.extend(points.iter().map(|p| p.0)),
            _ => {}
        }
        out
    }
}

fn waveform_at(w: &Waveform, t: f64, tstep: f64, tstop: f64) -> f64 {
    match *w {
        Waveform::Pulse { v1, v2, td, tr, tf, pw, per } => {
            let (tr, tf) = (tr.unwrap_or(tstep), tf.unwrap_or(tstep));
            let (pw, per) = (pw.unwrap_or(tstop), per.unwrap_or(tstop));
            if t < td {
                return v1;
            }
            let mut tt = t - td;
            if per > 0.0 && tt >= per {
                tt -= (tt / per).floor() * per;
            }
            if tt < tr {
                v1 + (v2 - v1) * tt / tr.max(f64::MIN_POSITIVE)
            } else if tt < tr + pw {
                v2
            } else if tt < tr + pw + tf {
                v2 + (v1 - v2) * (tt - tr - pw) / tf.max(f64::MIN_POSITIVE)
            } else {
                v1
            }
        }
        Waveform::Sin { vo, va, freq, td, theta, phase } => {
            let freq = freq.unwrap_or(1.0 / tstop);
            let ph = phase.to_radians();
            if t < td {
                vo + va * ph.sin()
            } else {
                let tt = t - td;
                vo + va * (-tt * theta).exp() * (std::f64::consts::TAU * freq * tt + ph).sin()
            }
        }
        Waveform::Pwl(ref points) => {
            let Some(first) = points.first() else { return 0.0 };
            if t <= first.0 {
                return first.1;
            }
            for w in points.windows(2) {
                if t <= w[1].0 {
                    let span = w[1].0 - w[0].0;
                    if span <= 0.0 {
                        return w[1].1;
                    }
                    return w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / span;
                }
            }
            points.last().expect("not empty").1
        }
    }
}

fn param(m: &ModelCard, key: &str, default: f64) -> f64 {
    m.params.get(key).copied().unwrap_or(default)
}

/// Diode model parameters (SPICE level 1).
#[derive(Debug, Clone, PartialEq)]
pub struct DiodeModel {
    pub is: f64,
    pub n: f64,
    pub rs: f64,
    pub cjo: f64,
    pub vj: f64,
    pub m: f64,
    pub fc: f64,
    pub tt: f64,
    pub bv: Option<f64>,
    pub ibv: f64,
    /// Emission coefficient in breakdown; defaults to `n`.
    pub nbv: f64,
}

impl DiodeModel {
    pub fn from_card(m: &ModelCard) -> Self {
        DiodeModel {
            is: param(m, "is", 1e-14),
            n: param(m, "n", 1.0),
            rs: param(m, "rs", 0.0),
            cjo: m.params.get("cjo").or(m.params.get("cj0")).copied().unwrap_or(0.0),
            vj: param(m, "vj", 1.0),
            m: param(m, "m", 0.5),
            fc: param(m, "fc", 0.5),
            tt: param(m, "tt", 0.0),
            bv: m.params.get("bv").copied(),
            ibv: param(m, "ibv", 1e-3),
            nbv: m.params.get("nbv").copied().unwrap_or_else(|| param(m, "n", 1.0)),
        }
    }
}

/// A junction diode, with its series resistance on an internal node.
#[derive(Debug, Clone, PartialEq)]
pub struct Diode {
    pub anode: usize,
    pub cathode: usize,
    /// The anode side of the junction: `anode` itself unless there is series resistance.
    pub junction: usize,
    pub area: f64,
    pub model: DiodeModel,
    /// Saturation current scaled by area.
    is: f64,
    vcrit: f64,
    /// The breakdown voltage adjusted so the current at `bv` is `ibv`.
    xbv: Option<f64>,
}

impl Diode {
    pub fn new(
        anode: usize,
        cathode: usize,
        junction: usize,
        area: f64,
        model: DiodeModel,
    ) -> Self {
        let is = model.is * area;
        let nvt = model.n * VT;
        let vcrit = nvt * (nvt / (std::f64::consts::SQRT_2 * is)).ln();
        let vtb = model.nbv * VT;
        let xbv = model.bv.map(|bv| {
            // ngspice dioTemp: find the knee so that I(-BV) = IBV.
            let cbv = model.ibv * area;
            if cbv < is * bv / VT {
                bv
            } else {
                let tol = 1e-3 * cbv;
                let mut xbv = bv - vtb * (1.0 + cbv / is).ln();
                for _ in 0..25 {
                    xbv = bv - vtb * (cbv / is + 1.0 - xbv / VT).ln();
                    let xcbv = is * (((bv - xbv) / vtb).exp() - 1.0 + xbv / VT);
                    if (xcbv - cbv).abs() <= tol {
                        break;
                    }
                }
                xbv
            }
        });
        Diode { anode, cathode, junction, area, model, is, vcrit, xbv }
    }

    /// Junction current and conductance at `vd`.
    pub fn iv(&self, vd: f64, gmin: f64) -> (f64, f64) {
        let nvt = self.model.n * VT;
        if vd >= -3.0 * nvt {
            let e = (vd / nvt).min(80.0).exp();
            (self.is * (e - 1.0) + gmin * vd, self.is * e / nvt + gmin)
        } else if self.xbv.is_none_or(|bv| vd >= -bv) {
            let arg = 3.0 * nvt / (vd * std::f64::consts::E);
            let arg = arg * arg * arg;
            (-self.is * (1.0 + arg) + gmin * vd, self.is * 3.0 * arg / vd + gmin)
        } else {
            let bv = self.xbv.expect("checked");
            let vtb = self.model.nbv * VT;
            let e = (-(bv + vd) / vtb).min(80.0).exp();
            (-self.is * e + gmin * vd, self.is * e / vtb + gmin)
        }
    }

    /// Limits a Newton step in junction voltage, forward and in breakdown, so the
    /// exponentials can't run away (SPICE3 `DEVpnjlim`, mirrored for breakdown).
    pub fn limit(&self, vnew: f64, vold: f64) -> (f64, bool) {
        let nvt = self.nvt();
        if let Some(bv) = self.xbv {
            let vtb = self.model.nbv * VT;
            if vnew < (-bv + 10.0 * vtb).min(0.0) {
                let (v, changed) = pnjlim(-(vnew + bv), -(vold + bv), vtb, self.vcrit);
                return (-(v + bv), changed);
            }
        }
        pnjlim(vnew, vold, nvt, self.vcrit)
    }

    /// Junction charge and capacitance at `vd`, given the junction current's
    /// conductance for diffusion charge.
    pub fn charge(&self, vd: f64, id: f64, gd: f64) -> (f64, f64) {
        let m = &self.model;
        let cjo = m.cjo * self.area;
        let (mut q, mut c) = (m.tt * id, m.tt * gd);
        if cjo > 0.0 {
            let fcv = m.fc * m.vj;
            if vd < fcv {
                let arg = 1.0 - vd / m.vj;
                let sarg = (-m.m * arg.ln()).exp();
                q += m.vj * cjo * (1.0 - arg * sarg) / (1.0 - m.m);
                c += cjo * sarg;
            } else {
                let f1 = m.vj * (1.0 - (1.0 - m.fc).powf(1.0 - m.m)) / (1.0 - m.m);
                let f2 = (1.0 - m.fc).powf(1.0 + m.m);
                let f3 = 1.0 - m.fc * (1.0 + m.m);
                let czof2 = cjo / f2;
                q += cjo * f1
                    + czof2 * (f3 * (vd - fcv) + (m.m / (m.vj + m.vj)) * (vd * vd - fcv * fcv));
                c += czof2 * (f3 + m.m * vd / m.vj);
            }
        }
        (q, c)
    }

    pub fn vcrit(&self) -> f64 {
        self.vcrit
    }

    pub fn nvt(&self) -> f64 {
        self.model.n * VT
    }
}

/// Bipolar transistor model parameters (SPICE Gummel-Poon, without excess phase,
/// quasi-saturation or substrate).
#[derive(Debug, Clone, PartialEq)]
pub struct BjtModel {
    /// +1 for NPN, -1 for PNP.
    pub polarity: f64,
    pub is: f64,
    pub bf: f64,
    pub nf: f64,
    pub vaf: f64,
    pub ikf: f64,
    pub ise: f64,
    pub ne: f64,
    pub br: f64,
    pub nr: f64,
    pub var: f64,
    pub ikr: f64,
    pub isc: f64,
    pub nc: f64,
    pub rb: f64,
    pub re: f64,
    pub rc: f64,
    pub cje: f64,
    pub vje: f64,
    pub mje: f64,
    pub cjc: f64,
    pub vjc: f64,
    pub mjc: f64,
    pub tf: f64,
    pub xtf: f64,
    pub vtf: f64,
    pub itf: f64,
    pub tr: f64,
    pub fc: f64,
}

impl BjtModel {
    pub fn from_card(m: &ModelCard) -> Result<Self, String> {
        let polarity = match m.kind.as_str() {
            "npn" => 1.0,
            "pnp" => -1.0,
            other => return Err(format!("`{other}` is not a bipolar transistor model type")),
        };
        // Zero means "infinite" for Early voltages, knee currents and VTF, as in SPICE.
        let inf = |k: &str| m.params.get(k).copied().filter(|v| *v != 0.0).unwrap_or(f64::INFINITY);
        Ok(BjtModel {
            polarity,
            is: param(m, "is", 1e-16),
            bf: param(m, "bf", 100.0),
            nf: param(m, "nf", 1.0),
            vaf: inf("vaf"),
            ikf: inf("ikf"),
            ise: param(m, "ise", 0.0),
            ne: param(m, "ne", 1.5),
            br: param(m, "br", 1.0),
            nr: param(m, "nr", 1.0),
            var: inf("var"),
            ikr: inf("ikr"),
            isc: param(m, "isc", 0.0),
            nc: param(m, "nc", 2.0),
            rb: param(m, "rb", 0.0),
            re: param(m, "re", 0.0),
            rc: param(m, "rc", 0.0),
            cje: param(m, "cje", 0.0),
            vje: param(m, "vje", 0.75),
            mje: param(m, "mje", 0.33),
            cjc: param(m, "cjc", 0.0),
            vjc: param(m, "vjc", 0.75),
            mjc: param(m, "mjc", 0.33),
            tf: param(m, "tf", 0.0),
            xtf: param(m, "xtf", 0.0),
            vtf: inf("vtf"),
            itf: param(m, "itf", 0.0),
            tr: param(m, "tr", 0.0),
            fc: param(m, "fc", 0.5),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bjt {
    pub c: usize,
    pub b: usize,
    pub e: usize,
    /// Internal nodes behind the series resistances (the terminal itself if none).
    pub ci: usize,
    pub bi: usize,
    pub ei: usize,
    pub area: f64,
    pub model: BjtModel,
}

/// Gummel-Poon terminal currents in normalised (NPN) voltages: currents into the
/// collector and base, and their derivatives (SPICE3 `bjtload`'s `gm`, `go`, `gpi`,
/// `gmu`), plus the junction charges.
#[derive(Debug, Clone, Copy, Default)]
pub struct BjtState {
    pub ic: f64,
    pub ib: f64,
    pub gm: f64,
    pub go: f64,
    pub gpi: f64,
    pub gmu: f64,
    pub qbe: f64,
    pub cbe: f64,
    pub qbc: f64,
    pub cbc: f64,
}

impl Bjt {
    pub fn vcrit(&self) -> f64 {
        VT * (VT / (std::f64::consts::SQRT_2 * self.model.is * self.area)).ln()
    }

    pub fn eval(&self, vbe: f64, vbc: f64, gmin: f64) -> BjtState {
        let m = &self.model;
        let csat = m.is * self.area;
        let (c2, c4) = (m.ise * self.area, m.isc * self.area);
        let junction = |v: f64, n: f64, c: f64, nl: f64, cl: f64| {
            let vtn = n * VT;
            let (i, g, il, gl) = if v >= -3.0 * vtn {
                let e = (v / vtn).min(80.0).exp();
                let (il, gl) = if cl == 0.0 {
                    (0.0, 0.0)
                } else {
                    let vte = nl * VT;
                    let el = (v / vte).min(80.0).exp();
                    (cl * (el - 1.0), cl * el / vte)
                };
                (c * (e - 1.0), c * e / vtn, il, gl)
            } else {
                let g = -c / v;
                let gl = -cl / v;
                (g * v, g, gl * v, gl)
            };
            (i, g, il + gmin * v, gl + gmin)
        };
        let (cbe, gbe, cben, gben) = junction(vbe, m.nf, csat, m.ne, c2);
        let (cbc, gbc, cbcn, gbcn) = junction(vbc, m.nr, csat, m.nc, c4);

        // Base charge: Early effect and high-level injection.
        let (ovaf, ovar) = (1.0 / m.vaf, 1.0 / m.var);
        let (oik, oikr) = (1.0 / (m.ikf * self.area), 1.0 / (m.ikr * self.area));
        let q1 = 1.0 / (1.0 - ovaf * vbc - ovar * vbe);
        let (qb, dqbdve, dqbdvc) = if oik == 0.0 && oikr == 0.0 {
            (q1, q1 * q1 * ovar, q1 * q1 * ovaf)
        } else {
            let q2 = oik * cbe + oikr * cbc;
            let arg = (1.0 + 4.0 * q2).max(0.0);
            let sqarg = if arg != 0.0 { arg.sqrt() } else { 1.0 };
            let qb = q1 * (1.0 + sqarg) / 2.0;
            (qb, q1 * (qb * ovar + oik * gbe / sqarg), q1 * (qb * ovaf + oikr * gbc / sqarg))
        };
        let ic = (cbe - cbc) / qb - cbc / m.br - cbcn;
        let ib = cbe / m.bf + cben + cbc / m.br + cbcn;
        let gpi = gbe / m.bf + gben;
        let gmu = gbc / m.br + gbcn;
        let go = (gbc + (cbe - cbc) * dqbdvc / qb) / qb;
        let gm = (gbe - (cbe - cbc) * dqbdve / qb) / qb - go;

        // Charges: transit time (modulated by XTF, VTF, ITF) plus depletion.
        let (mut cbe_t, mut gbe_t) = (cbe, gbe);
        if m.tf != 0.0 && vbe > 0.0 {
            let (mut argtf, mut arg2) = (0.0, 0.0);
            if m.xtf != 0.0 {
                argtf = m.xtf * (vbc / (1.44 * m.vtf)).exp();
                arg2 = argtf;
                if m.itf != 0.0 {
                    let xjtf = m.itf * self.area;
                    let temp = cbe / (cbe + xjtf);
                    argtf *= temp * temp;
                    arg2 = argtf * (3.0 - temp - temp);
                }
            }
            cbe_t = cbe * (1.0 + argtf) / qb;
            gbe_t = (gbe * (1.0 + arg2) - cbe_t * dqbdve) / qb;
        }
        let (qdbe, cdbe) = depletion(m.cje * self.area, m.vje, m.mje, m.fc, vbe);
        let (qdbc, cdbc) = depletion(m.cjc * self.area, m.vjc, m.mjc, m.fc, vbc);
        BjtState {
            ic,
            ib,
            gm,
            go,
            gpi,
            gmu,
            qbe: m.tf * cbe_t + qdbe,
            cbe: m.tf * gbe_t + cdbe,
            qbc: m.tr * cbc + qdbc,
            cbc: m.tr * gbc + cdbc,
        }
    }
}

/// MOSFET Level 1 (Shichman-Hodges) model parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct MosModel {
    /// +1 for NMOS, -1 for PMOS.
    pub polarity: f64,
    pub vto: f64,
    pub kp: f64,
    pub gamma: f64,
    pub phi: f64,
    pub lambda: f64,
    pub is: f64,
    pub pb: f64,
    pub cbd: f64,
    pub cbs: f64,
    pub mj: f64,
    pub fc: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    pub ld: f64,
    pub tox: Option<f64>,
    /// Bottom and sidewall junction capacitance per area and per perimeter, used when
    /// `CBD`/`CBS` aren't given.
    pub cj: f64,
    pub cjsw: f64,
    pub mjsw: f64,
}

/// Permittivity of silicon dioxide (3.9 ε₀), as SPICE uses it.
const EPS_OX: f64 = 3.9 * 8.854_214_871e-12;

impl MosModel {
    /// Gate oxide capacitance per area, or zero without `TOX`.
    pub fn cox(&self) -> f64 {
        self.tox.map_or(0.0, |t| EPS_OX / t)
    }
}

impl MosModel {
    pub fn from_card(m: &ModelCard) -> Result<Self, String> {
        let polarity = match m.kind.as_str() {
            "nmos" => 1.0,
            "pmos" => -1.0,
            other => return Err(format!("`{other}` is not a MOSFET model type")),
        };
        let level = param(m, "level", 1.0);
        if level != 1.0 {
            return Err(format!("MOSFET level {level} is not supported yet; use LEVEL=1"));
        }
        let tox = m.params.get("tox").copied();
        // Without KP, SPICE derives it from the mobility and the oxide.
        let kp = match (m.params.get("kp"), tox) {
            (Some(&kp), _) => kp,
            (None, Some(t)) => param(m, "uo", 600.0) * 1e-4 * EPS_OX / t,
            (None, None) => 2e-5,
        };
        Ok(MosModel {
            polarity,
            vto: param(m, "vto", 0.0),
            kp,
            gamma: param(m, "gamma", 0.0),
            phi: param(m, "phi", 0.6),
            lambda: param(m, "lambda", 0.0),
            is: param(m, "is", 1e-14),
            pb: param(m, "pb", 0.8),
            cbd: param(m, "cbd", 0.0),
            cbs: param(m, "cbs", 0.0),
            mj: param(m, "mj", 0.5),
            fc: param(m, "fc", 0.5),
            cgso: param(m, "cgso", 0.0),
            cgdo: param(m, "cgdo", 0.0),
            cgbo: param(m, "cgbo", 0.0),
            ld: param(m, "ld", 0.0),
            tox,
            cj: param(m, "cj", 0.0),
            cjsw: param(m, "cjsw", 0.0),
            mjsw: param(m, "mjsw", 0.5),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mosfet {
    pub d: usize,
    pub g: usize,
    pub s: usize,
    pub b: usize,
    pub w: f64,
    pub l: f64,
    /// Parallel device count.
    pub m: f64,
    /// Drain and source diffusion areas and perimeters, for junction capacitance.
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
    pub model: MosModel,
}

/// Meyer's gate capacitances (SPICE3 `DEVqmeyer`), as *half* values `(cgs, cgd, cgb)`:
/// SPICE averages the values at the two ends of each time step.
pub fn qmeyer(
    vgs: f64,
    vgd: f64,
    vgb: f64,
    von: f64,
    vdsat: f64,
    phi: f64,
    cox: f64,
) -> (f64, f64, f64) {
    let _ = vgb;
    let vgst = vgs - von;
    if vgst <= -phi {
        (0.0, 0.0, cox / 2.0)
    } else if vgst <= -phi / 2.0 {
        (0.0, 0.0, -vgst * cox / (2.0 * phi))
    } else if vgst <= 0.0 {
        (vgst * cox / (1.5 * phi) + cox / 3.0, 0.0, -vgst * cox / (2.0 * phi))
    } else {
        let vds = vgs - vgd;
        if vdsat <= vds {
            (cox / 3.0, 0.0, 0.0)
        } else {
            let vddif = 2.0 * vdsat - vds;
            let vddif1 = vdsat - vds;
            let vddif2 = vddif * vddif;
            (
                cox * (1.0 - vddif1 * vddif1 / vddif2) / 3.0,
                cox * (1.0 - vdsat * vdsat / vddif2) / 3.0,
                0.0,
            )
        }
    }
}

/// The channel current of a Level 1 MOSFET in normalised (n-channel) voltages, with
/// the device in forward mode (`vds ≥ 0`): `(id, gm, gds, gmbs, von)`.
pub fn mos1_channel(
    model: &MosModel,
    beta: f64,
    vgs: f64,
    vds: f64,
    vbs: f64,
) -> (f64, f64, f64, f64, f64) {
    let sphi = model.phi.sqrt();
    let sarg =
        if vbs <= 0.0 { (model.phi - vbs).sqrt() } else { (sphi - vbs / (2.0 * sphi)).max(0.0) };
    let von = model.polarity * model.vto + model.gamma * (sarg - sphi);
    let vgst = vgs - von;
    if vgst <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0, von);
    }
    let arg = if sarg > 0.0 { model.gamma / (2.0 * sarg) } else { 0.0 };
    let betap = beta * (1.0 + model.lambda * vds);
    if vgst <= vds {
        // Saturation.
        let id = betap * vgst * vgst * 0.5;
        let gm = betap * vgst;
        let gds = model.lambda * beta * vgst * vgst * 0.5;
        (id, gm, gds, gm * arg, von)
    } else {
        // Linear region.
        let id = betap * vds * (vgst - 0.5 * vds);
        let gm = betap * vds;
        let gds = betap * (vgst - vds) + model.lambda * beta * vds * (vgst - 0.5 * vds);
        (id, gm, gds, gm * arg, von)
    }
}

/// Bulk junction diode current and conductance (SPICE3 MOS1 form).
pub fn bulk_junction(is: f64, v: f64, gmin: f64) -> (f64, f64) {
    if v <= 0.0 {
        let g = is / VT;
        (g * v + gmin * v, g + gmin)
    } else {
        let e = (v / VT).min(80.0).exp();
        (is * (e - 1.0) + gmin * v, is * e / VT + gmin)
    }
}

/// Depletion charge and capacitance of a junction with zero-bias capacitance `cj`.
pub fn depletion(cj: f64, pb: f64, mj: f64, fc: f64, v: f64) -> (f64, f64) {
    if cj == 0.0 {
        return (0.0, 0.0);
    }
    let fcv = fc * pb;
    if v < fcv {
        let arg = 1.0 - v / pb;
        let sarg = (-mj * arg.ln()).exp();
        (pb * cj * (1.0 - arg * sarg) / (1.0 - mj), cj * sarg)
    } else {
        let f1 = pb * (1.0 - (1.0 - fc).powf(1.0 - mj)) / (1.0 - mj);
        let f2 = (1.0 - fc).powf(1.0 + mj);
        let f3 = 1.0 - fc * (1.0 + mj);
        let q = cj * f1 + cj / f2 * (f3 * (v - fcv) + mj / (2.0 * pb) * (v * v - fcv * fcv));
        (q, cj / f2 * (f3 + mj * v / pb))
    }
}

/// SPICE3 `DEVpnjlim`: limits a junction voltage step so the exponential stays tame.
pub fn pnjlim(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> (f64, bool) {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * vt {
        let v = if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 { vold + vt * arg.ln() } else { vcrit }
        } else {
            vt * (vnew / vt).ln()
        };
        (v, true)
    } else {
        (vnew, false)
    }
}

/// SPICE3 `DEVfetlim`: limits a gate voltage step around the threshold `vto`.
pub fn fetlim(vnew: f64, vold: f64, vto: f64) -> f64 {
    let vtsthi = (2.0 * (vold - vto)).abs() + 2.0;
    let vtstlo = vtsthi / 2.0 + 2.0;
    let vtox = vto + 3.5;
    let delv = vnew - vold;
    let mut v = vnew;
    if vold >= vto {
        if vold >= vtox {
            if delv <= 0.0 {
                if vnew >= vtox {
                    if -delv > vtstlo {
                        v = vold - vtstlo;
                    }
                } else {
                    v = vnew.max(vto + 2.0);
                }
            } else if delv >= vtsthi {
                v = vold + vtsthi;
            }
        } else if delv <= 0.0 {
            v = vnew.max(vto - 0.5);
        } else {
            v = vnew.min(vto + 4.0);
        }
    } else if delv <= 0.0 {
        if -delv > vtsthi {
            v = vold - vtsthi;
        }
    } else {
        let vtemp = vto + 0.5;
        if vnew <= vtemp {
            if delv > vtstlo {
                v = vold + vtstlo;
            }
        } else {
            v = vtemp;
        }
    }
    v
}

/// SPICE3 `DEVlimvds`: limits a drain-source voltage step.
pub fn limvds(vnew: f64, vold: f64) -> f64 {
    if vold >= 3.5 {
        if vnew > vold {
            vnew.min(3.0 * vold + 2.0)
        } else if vnew < 3.5 {
            vnew.max(2.0)
        } else {
            vnew
        }
    } else if vnew > vold {
        vnew.min(4.0)
    } else {
        vnew.max(-0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn card(kind: &str, params: &[(&str, f64)]) -> ModelCard {
        ModelCard {
            name: "m".into(),
            kind: kind.into(),
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>(),
            line: 1,
        }
    }

    #[test]
    fn diode_follows_shockley_and_breaks_down_at_bv() {
        let d = Diode::new(1, 0, 1, 1.0, DiodeModel::from_card(&card("d", &[("bv", 5.0)])));
        let (i, g) = d.iv(0.6, 0.0);
        assert!((i - 1e-14 * ((0.6 / VT).exp() - 1.0)).abs() < 1e-12 * i.abs());
        assert!(g > 0.0);
        let (ibv, _) = d.iv(-5.0, 0.0);
        assert!((ibv + 1e-3).abs() < 2e-5, "current at -BV is -IBV: {ibv}");
        let (rev, _) = d.iv(-1.0, 0.0);
        assert!(rev < 0.0 && rev > -2e-14);
    }

    #[test]
    fn mosfet_regions() {
        let m = MosModel::from_card(&card("nmos", &[("vto", 0.7), ("kp", 1e-4)])).unwrap();
        let beta = 1e-4;
        assert_eq!(mos1_channel(&m, beta, 0.5, 1.0, 0.0).0, 0.0, "off below threshold");
        let (sat, ..) = mos1_channel(&m, beta, 1.7, 2.0, 0.0);
        assert!((sat - 0.5e-4).abs() < 1e-12, "saturation: β/2 (Vgs-Vt)²");
        let (lin, ..) = mos1_channel(&m, beta, 1.7, 0.1, 0.0);
        assert!((lin - 1e-4 * 0.1 * (1.0 - 0.05)).abs() < 1e-12);
        assert!(MosModel::from_card(&card("nmos", &[("level", 3.0)])).is_err());
    }

    #[test]
    fn pulse_and_sine_waveforms() {
        let pulse = Source {
            spec: SourceSpec {
                tran: Some(Waveform::Pulse {
                    v1: 0.0,
                    v2: 1.0,
                    td: 1.0,
                    tr: Some(1.0),
                    tf: Some(1.0),
                    pw: Some(2.0),
                    per: Some(10.0),
                }),
                ..Default::default()
            },
        };
        let at = |t| pulse.at(t, 0.1, 20.0);
        assert_eq!((at(0.5), at(1.5), at(3.0), at(4.5), at(6.0)), (0.0, 0.5, 1.0, 0.5, 0.0));
        assert_eq!(at(13.0), 1.0, "periodic");
        assert_eq!(pulse.breakpoints(0.1, 12.0), [1.0, 2.0, 4.0, 5.0, 11.0, 12.0]);
        let sine = Source {
            spec: SourceSpec {
                tran: Some(Waveform::Sin {
                    vo: 1.0,
                    va: 2.0,
                    freq: Some(1.0),
                    td: 0.0,
                    theta: 0.0,
                    phase: 0.0,
                }),
                ..Default::default()
            },
        };
        assert!((sine.at(0.25, 0.1, 1.0) - 3.0).abs() < 1e-12);
        assert_eq!(sine.dc(), 1.0, "time-zero value when there is no DC value");
    }
}
