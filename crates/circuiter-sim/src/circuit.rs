//! A parsed deck turned into devices on numbered nodes, ready to simulate.

use crate::device::{Bjt, BjtModel, Diode, DiodeModel, Jfet, JfetModel, MosModel, Mosfet, Source};
use crate::parse::{Deck, parse};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Resistor {
        a: usize,
        b: usize,
        g: f64,
    },
    Capacitor {
        a: usize,
        b: usize,
        c: f64,
    },
    /// `mutual` lists the branch and mutual inductance of every coupled inductor.
    Inductor {
        a: usize,
        b: usize,
        l: f64,
        branch: usize,
        mutual: Vec<(usize, f64)>,
    },
    Vsource {
        p: usize,
        n: usize,
        branch: usize,
        source: Source,
    },
    Isource {
        p: usize,
        n: usize,
        source: Source,
    },
    Diode(Diode),
    Mosfet(Mosfet),
    Bjt(Bjt),
    Jfet(Jfet),
    /// `E`: a voltage between `p` and `n` of `gain` times the voltage from `cp` to `cn`.
    Vcvs {
        p: usize,
        n: usize,
        cp: usize,
        cn: usize,
        gain: f64,
        branch: usize,
    },
    /// `G`: a current from `p` through the source to `n` of `gm` times the voltage
    /// from `cp` to `cn`.
    Vccs {
        p: usize,
        n: usize,
        cp: usize,
        cn: usize,
        gm: f64,
    },
    /// `F`: a current of `gain` times the current in branch `control`.
    Cccs {
        p: usize,
        n: usize,
        control: usize,
        gain: f64,
    },
    /// `H`: a voltage of `r` times the current in branch `control`.
    Ccvs {
        p: usize,
        n: usize,
        control: usize,
        r: f64,
        branch: usize,
    },
    /// A switch (`S`, `W`): `ron` when on, `roff` when off. It turns on when its control
    /// rises above `threshold + hysteresis`, off when it falls below `threshold -
    /// hysteresis`, and otherwise stays as it was.
    Switch {
        p: usize,
        n: usize,
        control: SwitchControl,
        ron: f64,
        roff: f64,
        threshold: f64,
        hysteresis: f64,
        initially_on: bool,
    },
    /// A destroyed part: nothing, or a short between two nodes. A part that had a
    /// branch current keeps the unknown, held at zero, so the equations stay square.
    Failed {
        branch: Option<usize>,
        short: Option<(usize, usize, f64)>,
    },
    /// A SPICE2 polynomial source: an output voltage (E, H; `branch` is `Some`) or
    /// current (G, F) that is a polynomial in its controlling voltages or currents.
    Poly {
        p: usize,
        n: usize,
        branch: Option<usize>,
        inputs: Vec<PolyInput>,
        poly: Law,
    },
}

/// What a controlled source computes from its inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum Law {
    /// SPICE2 `POLY`.
    Poly(Polynomial),
    /// A `B` source's expression, which may also depend on time.
    Expr(std::sync::Arc<crate::behave::Expr>),
}

impl Law {
    /// The value and its partial derivatives at inputs `x` and time `t`.
    pub fn eval(&self, x: &[f64], t: f64) -> (f64, Vec<f64>) {
        match self {
            Law::Poly(p) => p.eval(x),
            Law::Expr(e) => e.eval(x, t),
        }
    }
}

/// What a switch watches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwitchControl {
    Voltage(usize, usize),
    /// Resolved once every element exists.
    Current(usize),
}

/// A controlling input of a polynomial source.
#[derive(Debug, Clone, PartialEq)]
pub enum PolyInput {
    Voltage(usize, usize),
    /// Resolved once every element exists.
    Current(usize),
}

/// `p0 + p1 x1 + ... + pn xn + p(n+1) x1² + p(n+2) x1 x2 + ...`: coefficients in
/// SPICE's order, every product of the inputs by degree, then lexicographically.
#[derive(Debug, Clone, PartialEq)]
pub struct Polynomial {
    pub coeffs: Vec<f64>,
    /// The input indices multiplied in each term after the constant.
    terms: Vec<Vec<usize>>,
}

impl Polynomial {
    pub fn new(dims: usize, coeffs: Vec<f64>) -> Self {
        let mut terms: Vec<Vec<usize>> = Vec::new();
        let mut degree = 1;
        while terms.len() + 1 < coeffs.len() {
            // Non-decreasing index sequences of this degree, in lexicographic order.
            let mut idx = vec![0; degree];
            loop {
                terms.push(idx.clone());
                let mut k = degree;
                while k > 0 && idx[k - 1] == dims - 1 {
                    k -= 1;
                }
                if k == 0 {
                    break;
                }
                idx[k - 1] += 1;
                let v = idx[k - 1];
                idx[k..].iter_mut().for_each(|x| *x = v);
            }
            degree += 1;
        }
        Polynomial { coeffs, terms }
    }

    /// The value and its partial derivatives at `x`.
    pub fn eval(&self, x: &[f64]) -> (f64, Vec<f64>) {
        let mut value = self.coeffs.first().copied().unwrap_or(0.0);
        let mut grad = vec![0.0; x.len()];
        for (c, term) in self.coeffs.iter().skip(1).zip(&self.terms) {
            if *c == 0.0 {
                continue;
            }
            value += c * term.iter().map(|&i| x[i]).product::<f64>();
            for (k, &i) in term.iter().enumerate() {
                let others: f64 =
                    term.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, &m)| x[m]).product();
                grad[i] += c * others;
            }
        }
        (value, grad)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// As written in the deck, e.g. `MU1.MN`.
    pub name: String,
    pub element: Element,
    /// First charge-storage slot (capacitances, inductor flux) of this device.
    pub slot: usize,
}

impl Element {
    /// The branch-current unknown this element adds, if any.
    pub fn branch(&self) -> Option<usize> {
        match self {
            Element::Inductor { branch, .. }
            | Element::Vsource { branch, .. }
            | Element::Vcvs { branch, .. }
            | Element::Ccvs { branch, .. } => Some(*branch),
            Element::Poly { branch, .. } | Element::Failed { branch, .. } => *branch,
            _ => None,
        }
    }
}

impl Device {
    /// The external nodes this device connects to, in its card's order.
    pub fn terminals(&self) -> Vec<usize> {
        match &self.element {
            Element::Resistor { a, b, .. }
            | Element::Capacitor { a, b, .. }
            | Element::Inductor { a, b, .. } => vec![*a, *b],
            Element::Vsource { p, n, .. } | Element::Isource { p, n, .. } => vec![*p, *n],
            Element::Diode(d) => vec![d.anode, d.cathode],
            Element::Mosfet(m) => vec![m.d, m.g, m.s, m.b],
            Element::Bjt(q) => vec![q.c, q.b, q.e],
            Element::Jfet(j) => vec![j.d, j.g, j.s],
            Element::Vcvs { p, n, cp, cn, .. } | Element::Vccs { p, n, cp, cn, .. } => {
                vec![*p, *n, *cp, *cn]
            }
            Element::Cccs { p, n, .. } | Element::Ccvs { p, n, .. } => vec![*p, *n],
            Element::Failed { short, .. } => short.map_or(Vec::new(), |(a, b, _)| vec![a, b]),
            Element::Switch { p, n, control, .. } => match control {
                SwitchControl::Voltage(a, b) => vec![*p, *n, *a, *b],
                SwitchControl::Current(_) => vec![*p, *n],
            },
            Element::Poly { p, n, inputs, .. } => {
                let mut t = vec![*p, *n];
                for input in inputs {
                    if let PolyInput::Voltage(a, b) = input {
                        t.extend([*a, *b]);
                    }
                }
                t
            }
        }
    }

    pub(crate) fn slot_count(element: &Element) -> usize {
        match element {
            Element::Capacitor { .. }
            | Element::Inductor { .. }
            | Element::Diode(_)
            | Element::Switch { .. } => 1,
            Element::Mosfet(_) => 5,
            Element::Bjt(_) | Element::Jfet(_) => 2,
            _ => 0,
        }
    }
}

/// A circuit ready to simulate. Node 0 is ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Circuit {
    /// Node names as first written; index 0 is `0`. Internal nodes (series resistance)
    /// are named `<device>#<terminal>`.
    pub nodes: Vec<String>,
    index: HashMap<String, usize>,
    pub devices: Vec<Device>,
    /// Branch-current unknowns: voltage sources and inductors.
    pub branches: Vec<String>,
    pub slots: usize,
    /// Things the simulation will approximate or ignore, worth telling the user.
    pub warnings: Vec<String>,
}

impl Circuit {
    /// Parses a SPICE deck and builds the circuit.
    pub fn parse(deck: &str) -> Result<Circuit, String> {
        Circuit::from_deck(&parse(deck)?)
    }

    pub fn from_deck(deck: &Deck) -> Result<Circuit, String> {
        let mut c = Circuit {
            nodes: vec!["0".to_owned()],
            index: HashMap::from([("0".to_owned(), 0), ("gnd".to_owned(), 0)]),
            devices: Vec::new(),
            branches: Vec::new(),
            slots: 0,
            warnings: Vec::new(),
        };
        let models: HashMap<&str, &crate::parse::ModelCard> =
            deck.models.iter().map(|m| (m.name.as_str(), m)).collect();
        let mut names = HashMap::new();
        for card in &deck.cards {
            let at = |e: String| format!("line {}: {}: {e}", card.line, card.name);
            if let Some(line) = names.insert(card.name.to_ascii_lowercase(), card.line) {
                return Err(at(format!("name already used on line {line}")));
            }
            let nodes: Vec<usize> = card.nodes.iter().map(|n| c.node_or_insert(n)).collect();
            let first = |k: usize| card.values.get(k).copied();
            let element = match card.kind() {
                'R' => {
                    let r = first(0).ok_or_else(|| at("needs a resistance".into()))?;
                    if r == 0.0 {
                        return Err(at("zero resistance; use a wire or a 0 V source".into()));
                    }
                    Element::Resistor { a: nodes[0], b: nodes[1], g: 1.0 / r }
                }
                'C' => Element::Capacitor {
                    a: nodes[0],
                    b: nodes[1],
                    c: first(0).ok_or_else(|| at("needs a capacitance".into()))?,
                },
                'L' => Element::Inductor {
                    a: nodes[0],
                    b: nodes[1],
                    l: first(0).ok_or_else(|| at("needs an inductance".into()))?,
                    branch: c.branch(&card.name),
                    mutual: Vec::new(),
                },
                'Q' => {
                    let known = |w: &String| models.contains_key(w.to_ascii_lowercase().as_str());
                    let (model_word, rest) = match card.refs.iter().position(known) {
                        Some(0) => (&card.refs[0], &card.refs[1..]),
                        // A substrate node comes first; it isn't modelled.
                        Some(1) => (&card.refs[1], &card.refs[2..]),
                        _ => return Err(at("needs a known .model".into())),
                    };
                    let m = models[model_word.to_ascii_lowercase().as_str()];
                    let model = BjtModel::from_card(m).map_err(at)?;
                    let area = match rest.first() {
                        Some(a) => crate::parse::value(a).map_err(at)?,
                        None => card.params.get("area").copied().unwrap_or(1.0),
                    };
                    let mut internal = |r: f64, node: usize, tag: &str| {
                        if r > 0.0 { c.internal(&format!("{}#{tag}", card.name)) } else { node }
                    };
                    let ci = internal(model.rc, nodes[0], "c");
                    let bi = internal(model.rb, nodes[1], "b");
                    let ei = internal(model.re, nodes[2], "e");
                    Element::Bjt(Bjt {
                        c: nodes[0],
                        b: nodes[1],
                        e: nodes[2],
                        ci,
                        bi,
                        ei,
                        area,
                        model,
                    })
                }
                'S' | 'W' => {
                    let name = card.model.as_deref().unwrap_or_default();
                    let m = models.get(name).ok_or_else(|| at(format!("no model `{name}`")))?;
                    let (want, t, h) =
                        if card.kind() == 'S' { ("sw", "vt", "vh") } else { ("csw", "it", "ih") };
                    if m.kind != want {
                        return Err(at(format!(
                            "model `{name}` is not a {} model",
                            want.to_ascii_uppercase()
                        )));
                    }
                    let p = |k: &str, d: f64| m.params.get(k).copied().unwrap_or(d);
                    Element::Switch {
                        p: nodes[0],
                        n: nodes[1],
                        control: if card.kind() == 'S' {
                            SwitchControl::Voltage(nodes[2], nodes[3])
                        } else {
                            SwitchControl::Current(usize::MAX)
                        },
                        ron: p("ron", 1.0),
                        roff: p("roff", 1e12),
                        threshold: p(t, 0.0),
                        hysteresis: p(h, 0.0),
                        initially_on: card.params.contains_key("on"),
                    }
                }
                'E' | 'G' | 'F' | 'H' if card.poly.is_some() => {
                    let dims = card.poly.expect("checked");
                    let inputs = if matches!(card.kind(), 'E' | 'G') {
                        (0..dims)
                            .map(|k| PolyInput::Voltage(nodes[2 + 2 * k], nodes[3 + 2 * k]))
                            .collect()
                    } else {
                        vec![PolyInput::Current(usize::MAX); dims]
                    };
                    let branch = matches!(card.kind(), 'E' | 'H').then(|| c.branch(&card.name));
                    Element::Poly {
                        p: nodes[0],
                        n: nodes[1],
                        branch,
                        inputs,
                        poly: Law::Poly(Polynomial::new(dims, card.values.clone())),
                    }
                }
                'B' => {
                    let b =
                        card.behavior.as_ref().ok_or_else(|| at("needs an expression".into()))?;
                    let mut inputs: Vec<PolyInput> = b
                        .voltages
                        .iter()
                        .map(|(p, n)| PolyInput::Voltage(c.node_or_insert(p), c.node_or_insert(n)))
                        .collect();
                    inputs.extend(b.currents.iter().map(|_| PolyInput::Current(usize::MAX)));
                    let branch = (!b.current).then(|| c.branch(&card.name));
                    Element::Poly {
                        p: nodes[0],
                        n: nodes[1],
                        branch,
                        inputs,
                        poly: Law::Expr(std::sync::Arc::new(b.expr.clone())),
                    }
                }
                'E' => Element::Vcvs {
                    p: nodes[0],
                    n: nodes[1],
                    cp: nodes[2],
                    cn: nodes[3],
                    gain: first(0).ok_or_else(|| at("needs a gain".into()))?,
                    branch: c.branch(&card.name),
                },
                'G' => Element::Vccs {
                    p: nodes[0],
                    n: nodes[1],
                    cp: nodes[2],
                    cn: nodes[3],
                    gm: first(0).ok_or_else(|| at("needs a transconductance".into()))?,
                },
                // The controlling source is resolved once every element exists.
                'F' => Element::Cccs {
                    p: nodes[0],
                    n: nodes[1],
                    control: usize::MAX,
                    gain: card.values[0],
                },
                'H' => Element::Ccvs {
                    p: nodes[0],
                    n: nodes[1],
                    control: usize::MAX,
                    r: card.values[0],
                    branch: c.branch(&card.name),
                },
                // Couplings are applied once every inductor exists.
                'K' => continue,
                'V' => Element::Vsource {
                    p: nodes[0],
                    n: nodes[1],
                    branch: c.branch(&card.name),
                    source: Source { spec: card.source.clone().unwrap_or_default() },
                },
                'I' => Element::Isource {
                    p: nodes[0],
                    n: nodes[1],
                    source: Source { spec: card.source.clone().unwrap_or_default() },
                },
                'D' => {
                    let name = card.model.as_deref().unwrap_or_default();
                    let m = models.get(name).ok_or_else(|| at(format!("no model `{name}`")))?;
                    if m.kind != "d" {
                        return Err(at(format!("model `{name}` is not a diode model")));
                    }
                    let model = DiodeModel::from_card(m);
                    let junction = if model.rs > 0.0 {
                        c.internal(&format!("{}#a", card.name))
                    } else {
                        nodes[0]
                    };
                    let area = first(0).unwrap_or(1.0);
                    Element::Diode(Diode::new(nodes[0], nodes[1], junction, area, model))
                }
                'J' => {
                    let name = card.model.as_deref().unwrap_or_default();
                    let m = models.get(name).ok_or_else(|| at(format!("no model `{name}`")))?;
                    let model = JfetModel::from_card(m).map_err(at)?;
                    let mut internal = |r: f64, node: usize, tag: &str| {
                        if r > 0.0 { c.internal(&format!("{}#{tag}", card.name)) } else { node }
                    };
                    let di = internal(model.rd, nodes[0], "d");
                    let si = internal(model.rs, nodes[2], "s");
                    Element::Jfet(Jfet {
                        d: nodes[0],
                        g: nodes[1],
                        s: nodes[2],
                        di,
                        si,
                        area: first(0).unwrap_or(1.0),
                        model,
                    })
                }
                'M' => {
                    let name = card.model.as_deref().unwrap_or_default();
                    let m = models.get(name).ok_or_else(|| at(format!("no model `{name}`")))?;
                    let model = MosModel::from_card(m).map_err(at)?;
                    let p = |k: &str, d: f64| card.params.get(k).copied().unwrap_or(d);
                    Element::Mosfet(Mosfet {
                        d: nodes[0],
                        g: nodes[1],
                        s: nodes[2],
                        b: nodes[3],
                        w: p("w", 100e-6),
                        l: p("l", 100e-6),
                        m: p("m", 1.0),
                        ad: p("ad", 0.0),
                        as_: p("as", 0.0),
                        pd: p("pd", 0.0),
                        ps: p("ps", 0.0),
                        model,
                    })
                }
                other => return Err(at(format!("`{other}` elements are not supported"))),
            };
            let slot = c.slots;
            c.slots += Device::slot_count(&element);
            c.devices.push(Device { name: card.name.clone(), element, slot });
        }
        for card in deck.cards.iter().filter(|c| matches!(c.kind(), 'F' | 'H' | 'W' | 'B')) {
            let at = |e: String| format!("line {}: {}: {e}", card.line, card.name);
            let mut controls = Vec::new();
            for r in &card.refs {
                controls.push(
                    c.branch_of(r)
                        .ok_or_else(|| at(format!("`{r}` is not a voltage source or inductor")))?,
                );
            }
            let d = c.device(&card.name).expect("added above");
            match &mut c.devices[d].element {
                Element::Cccs { control: k, .. } | Element::Ccvs { control: k, .. } => {
                    *k = controls[0]
                }
                Element::Poly { inputs, .. } => {
                    let unresolved =
                        inputs.iter_mut().filter(|i| **i == PolyInput::Current(usize::MAX));
                    for (input, b) in unresolved.zip(controls) {
                        *input = PolyInput::Current(b);
                    }
                }
                Element::Switch { control, .. } => *control = SwitchControl::Current(controls[0]),
                _ => {}
            }
        }
        for card in deck.cards.iter().filter(|c| c.kind() == 'K') {
            let at = |e: String| format!("line {}: {}: {e}", card.line, card.name);
            let k = card.values[0];
            if !(k.abs() > 0.0 && k.abs() <= 1.0) {
                return Err(at(format!("coupling {k} must be between -1 and 1, and not 0")));
            }
            let inductor = |name: &str| -> Result<(usize, usize, f64), String> {
                let d = c.device(name).ok_or_else(|| at(format!("no inductor `{name}`")))?;
                match &c.devices[d].element {
                    Element::Inductor { l, branch, .. } => Ok((d, *branch, *l)),
                    _ => Err(at(format!("`{name}` is not an inductor"))),
                }
            };
            let (d1, b1, l1) = inductor(&card.refs[0])?;
            let (d2, b2, l2) = inductor(&card.refs[1])?;
            if d1 == d2 {
                return Err(at("an inductor can't be coupled to itself".into()));
            }
            let m = k * (l1 * l2).sqrt();
            for (d, other) in [(d1, b2), (d2, b1)] {
                if let Element::Inductor { mutual, .. } = &mut c.devices[d].element {
                    mutual.push((other, m));
                }
            }
        }
        Ok(c)
    }

    fn node_or_insert(&mut self, name: &str) -> usize {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        self.nodes.push(name.to_owned());
        self.index.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    fn internal(&mut self, name: &str) -> usize {
        self.node_or_insert(name)
    }

    fn branch(&mut self, name: &str) -> usize {
        self.branches.push(name.to_owned());
        self.branches.len() - 1
    }

    /// For each storage slot, whether it holds a charge or flux (whose truncation
    /// error limits the time step) rather than a state, such as a switch's.
    pub fn charge_slots(&self) -> Vec<bool> {
        let mut out = vec![true; self.slots];
        for d in &self.devices {
            if let Element::Switch { .. } = d.element {
                out[d.slot] = false;
            }
        }
        out
    }

    /// The node with this name, ignoring case. `0` and `gnd` are ground.
    pub fn node(&self, name: &str) -> Option<usize> {
        self.index.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn device(&self, name: &str) -> Option<usize> {
        self.devices.iter().position(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// The branch current unknown of a voltage source or inductor.
    pub fn branch_of(&self, name: &str) -> Option<usize> {
        self.branches.iter().position(|b| b.eq_ignore_ascii_case(name))
    }

    /// Structural problems that make the equations singular or the answer
    /// meaningless: nodes with no DC path to ground, and loops of voltage sources and
    /// inductors. Each problem names the nodes or devices involved.
    pub fn topology_problems(&self) -> Vec<Problem> {
        let n = self.nodes.len();
        let mut dc = UnionFind::new(n);
        let mut stiff = UnionFind::new(n);
        let mut out = Vec::new();
        for dev in &self.devices {
            match &dev.element {
                Element::Resistor { a, b, .. } => {
                    dc.union(*a, *b);
                }
                Element::Inductor { a, b, .. } | Element::Vsource { p: a, n: b, .. } => {
                    dc.union(*a, *b);
                    if !stiff.union(*a, *b) {
                        out.push(Problem::SourceLoop { device: dev.name.clone() });
                    }
                }
                Element::Diode(d) => {
                    dc.union(d.anode, d.junction);
                    dc.union(d.junction, d.cathode);
                }
                Element::Mosfet(m) => {
                    // Channel and bulk junctions conduct; the gate is insulated.
                    dc.union(m.d, m.s);
                    dc.union(m.d, m.b);
                }
                Element::Jfet(j) => {
                    for (a, b) in [(j.d, j.di), (j.s, j.si), (j.di, j.si), (j.g, j.si)] {
                        dc.union(a, b);
                    }
                }
                Element::Bjt(q) => {
                    for (a, b) in
                        [(q.c, q.ci), (q.b, q.bi), (q.e, q.ei), (q.ci, q.bi), (q.bi, q.ei)]
                    {
                        dc.union(a, b);
                    }
                }
                Element::Vcvs { p, n, .. } | Element::Ccvs { p, n, .. } => {
                    dc.union(*p, *n);
                    if !stiff.union(*p, *n) {
                        out.push(Problem::SourceLoop { device: dev.name.clone() });
                    }
                }
                Element::Poly { p, n, branch: Some(_), .. } => {
                    dc.union(*p, *n);
                    if !stiff.union(*p, *n) {
                        out.push(Problem::SourceLoop { device: dev.name.clone() });
                    }
                }
                Element::Switch { p, n, .. } => {
                    dc.union(*p, *n);
                }
                Element::Failed { short, .. } => {
                    if let Some((a, b, _)) = short {
                        dc.union(*a, *b);
                    }
                }
                Element::Poly { .. }
                | Element::Capacitor { .. }
                | Element::Isource { .. }
                | Element::Vccs { .. }
                | Element::Cccs { .. } => {}
            }
        }
        let floating: Vec<String> =
            (1..n).filter(|&k| dc.find(k) != dc.find(0)).map(|k| self.nodes[k].clone()).collect();
        if !floating.is_empty() {
            out.push(Problem::Floating { nodes: floating });
        }
        out
    }
}

/// A structural problem found before simulating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// These nodes have no DC path to ground (through resistors, inductors, sources,
    /// junctions or channels). Only capacitors, current sources or gates reach them.
    Floating { nodes: Vec<String> },
    /// This voltage source or inductor closes a loop of voltage sources and inductors,
    /// which would force a voltage around the loop with nothing to limit the current.
    SourceLoop { device: String },
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n).collect())
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }

    /// Joins two sets; false if they were already one.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return false;
        }
        self.0[a] = b;
        true
    }
}
