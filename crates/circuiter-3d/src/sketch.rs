//! Drawings: sketches made of points, lines, arcs and circles held by constraints and
//! driving dimensions, as in a 2D CAD program. The constraints are met by damped
//! Gauss-Newton over the points' coordinates (and circles' radii); the closed outlines
//! are found by walking the drawing's graph, each connected piece's outside boundary
//! taken as one outline. Also the drawing operations (trim, extend, offset, mirror,
//! corner fillets and chamfers), object snaps, and typed coordinates (`@20<45`).

use circuiter_model::model3d::{Constraint, Curve, Drawing, Sketch, fmt};

/// A point in the sketch plane, in millimetres.
pub type P = [f64; 2];
type Vars<'a> = &'a dyn Fn(&str) -> Option<f64>;

/// How close counts as the same place, in millimetres.
pub const TOL: f64 = 1e-4;

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: P, b: P) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn len(a: P) -> f64 {
    a[0].hypot(a[1])
}
fn dist(a: P, b: P) -> f64 {
    len(sub(a, b))
}
fn unit(a: P) -> P {
    let l = len(a).max(1e-12);
    [a[0] / l, a[1] / l]
}
fn angle_of(a: P) -> f64 {
    a[1].atan2(a[0])
}
/// `a` brought into [0, 2π).
fn wrap(a: f64) -> f64 {
    a.rem_euclid(std::f64::consts::TAU)
}

/// A drawing's numbers: each point's place, and each curve's radius (circles only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    pub points: Vec<P>,
    pub radii: Vec<f64>,
}

/// [`crate::cad::value`] in double precision.
fn eval(text: &str, vars: Vars) -> Result<f64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("a value is missing".into());
    }
    circuiter_sim::number::eval_with(t, vars).map_err(|e| format!("`{t}`: {e}"))
}

fn plain(text: &str) -> bool {
    text.trim().parse::<f64>().is_ok()
}

/// The drawing's numbers as written, before constraints.
pub fn geometry(d: &Drawing, vars: Vars) -> Result<Geometry, String> {
    let points = d
        .points
        .iter()
        .map(|[x, y]| Ok([eval(x, vars)?, eval(y, vars)?]))
        .collect::<Result<Vec<P>, String>>()?;
    let mut radii = vec![0.0; d.curves.len()];
    for (k, c) in d.curves.iter().enumerate() {
        let refs: &[usize] = match c {
            Curve::Line(a, b) | Curve::Guide(a, b) => &[*a, *b],
            Curve::Arc { center, from, to } => &[*center, *from, *to],
            Curve::Circle { center, radius } => {
                radii[k] = eval(radius, vars)?;
                &[*center]
            }
        };
        if refs.iter().any(|&i| i >= points.len()) {
            return Err(format!("curve {} names a point that isn't there", k + 1));
        }
    }
    Ok(Geometry { points, radii })
}

/// A curve as numbers: a segment, or an arc from angle `a0` sweeping `sweep` radians
/// counter-clockwise (a whole turn for circles).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prim {
    Seg(P, P),
    Arc { c: P, r: f64, a0: f64, sweep: f64 },
}

/// Curve `k` as numbers.
pub fn prim(d: &Drawing, g: &Geometry, k: usize) -> Prim {
    let p = |i: usize| g.points[i];
    match &d.curves[k] {
        Curve::Line(a, b) | Curve::Guide(a, b) => Prim::Seg(p(*a), p(*b)),
        Curve::Arc { center, from, to } => {
            let c = p(*center);
            let a0 = angle_of(sub(p(*from), c));
            let mut sweep = wrap(angle_of(sub(p(*to), c)) - a0);
            if from == to || sweep < 1e-9 {
                sweep = std::f64::consts::TAU;
            }
            Prim::Arc { c, r: dist(p(*from), c), a0, sweep }
        }
        Curve::Circle { center, .. } => {
            Prim::Arc { c: p(*center), r: g.radii[k], a0: 0.0, sweep: std::f64::consts::TAU }
        }
    }
}

impl Prim {
    /// The point at parameter `t`: along a segment 0..1, round an arc 0..sweep.
    pub fn at(&self, t: f64) -> P {
        match *self {
            Prim::Seg(a, b) => add(a, mul(sub(b, a), t)),
            Prim::Arc { c, r, a0, .. } => [c[0] + r * (a0 + t).cos(), c[1] + r * (a0 + t).sin()],
        }
    }

    /// The parameter of the point nearest `q`, clamped to the curve.
    pub fn param(&self, q: P) -> f64 {
        match *self {
            Prim::Seg(a, b) => {
                let ab = sub(b, a);
                (dot(sub(q, a), ab) / dot(ab, ab).max(1e-18)).clamp(0.0, 1.0)
            }
            Prim::Arc { c, a0, sweep, .. } => {
                let t = wrap(angle_of(sub(q, c)) - a0);
                if t <= sweep {
                    t
                } else if t - sweep < std::f64::consts::TAU - t {
                    sweep
                } else {
                    0.0
                }
            }
        }
    }

    /// The parameter where the curve ends.
    pub fn end(&self) -> f64 {
        match *self {
            Prim::Seg(..) => 1.0,
            Prim::Arc { sweep, .. } => sweep,
        }
    }

    pub fn nearest(&self, q: P) -> P {
        self.at(self.param(q))
    }

    /// Whether this is a whole circle (no ends).
    fn closed(&self) -> bool {
        matches!(*self, Prim::Arc { sweep, .. } if sweep >= std::f64::consts::TAU - 1e-9)
    }

    /// Whether parameter `t` lies on the curve.
    fn holds(&self, t: f64) -> bool {
        let e = match *self {
            Prim::Seg(a, b) => 1e-9 + TOL / dist(a, b).max(1e-9),
            Prim::Arc { r, .. } => 1e-9 + TOL / r.max(1e-9),
        };
        t >= -e && t <= self.end() + e
    }

    /// Points along the curve, ends included, close enough to look smooth.
    pub fn points(&self) -> Vec<P> {
        match *self {
            Prim::Seg(a, b) => vec![a, b],
            Prim::Arc { sweep, .. } => {
                let n = ((sweep * 24.0 / std::f64::consts::PI).ceil() as usize).max(4);
                (0..=n).map(|k| self.at(sweep * k as f64 / n as f64)).collect()
            }
        }
    }
}

/// Where two curves meet, as parameters on each. With `extend_a`, `a` counts as its
/// whole line or circle.
pub fn hits(a: &Prim, b: &Prim, extend_a: bool) -> Vec<(f64, f64)> {
    let raw: Vec<P> = match (*a, *b) {
        (Prim::Seg(p, q), Prim::Seg(r, s)) => {
            let (d1, d2) = (sub(q, p), sub(s, r));
            let den = cross(d1, d2);
            if den.abs() < 1e-12 {
                return Vec::new();
            }
            let t = cross(sub(r, p), d2) / den;
            vec![a.at(t)]
        }
        (Prim::Seg(p, q), Prim::Arc { c, r, .. }) | (Prim::Arc { c, r, .. }, Prim::Seg(p, q)) => {
            line_circle(p, q, c, r)
        }
        (Prim::Arc { c: c1, r: r1, .. }, Prim::Arc { c: c2, r: r2, .. }) => {
            circle_circle(c1, r1, c2, r2)
        }
    };
    raw.into_iter()
        .filter_map(|x| {
            let ta = match *a {
                Prim::Seg(p, q) => {
                    let pq = sub(q, p);
                    dot(sub(x, p), pq) / dot(pq, pq).max(1e-18)
                }
                Prim::Arc { c, a0, .. } => wrap(angle_of(sub(x, c)) - a0),
            };
            let tb = b.param(x);
            let on_b = dist(b.at(tb), x) < TOL * 10.0;
            let on_a = extend_a || a.holds(ta);
            (on_a && on_b).then_some((ta, tb))
        })
        .collect()
}

fn line_circle(p: P, q: P, c: P, r: f64) -> Vec<P> {
    let d = unit(sub(q, p));
    let f = sub(p, c);
    let b = dot(f, d);
    let disc = b * b - (dot(f, f) - r * r);
    if disc < -1e-9 {
        return Vec::new();
    }
    let s = disc.max(0.0).sqrt();
    let mut v = vec![add(p, mul(d, -b - s))];
    if s > 1e-9 {
        v.push(add(p, mul(d, -b + s)));
    }
    v
}

fn circle_circle(c1: P, r1: f64, c2: P, r2: f64) -> Vec<P> {
    let d = dist(c1, c2);
    if d < 1e-12 || d > r1 + r2 + 1e-9 || d < (r1 - r2).abs() - 1e-9 {
        return Vec::new();
    }
    let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
    let h = (r1 * r1 - a * a).max(0.0).sqrt();
    let u = unit(sub(c2, c1));
    let m = add(c1, mul(u, a));
    let n = [-u[1], u[0]];
    let mut v = vec![add(m, mul(n, h))];
    if h > 1e-9 {
        v.push(add(m, mul(n, -h)));
    }
    v
}

// ---- Constraints ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Var {
    X(usize),
    Y(usize),
    R(usize),
}

/// The drawing's numbers with its constraints met as nearly as they can be, keeping
/// the points in `pinned` where they are, and how far from met they are (0 when all
/// are).
pub fn solve(d: &Drawing, vars: Vars, pinned: &[usize]) -> Result<(Geometry, f64), String> {
    let mut g = geometry(d, vars)?;
    // Dimensions' values, and the sense each tangency and offset keeps.
    let mut values = Vec::with_capacity(d.constraints.len());
    for c in &d.constraints {
        let check = |i: usize, points: bool| {
            let ok = if points { i < g.points.len() } else { i < d.curves.len() };
            if ok {
                Ok(())
            } else {
                Err(format!("a constraint names something that isn't there: {c:?}"))
            }
        };
        use Constraint::*;
        match c {
            Coincident(a, b) | Distance(a, b, _) | DistanceX(a, b, _) | DistanceY(a, b, _) => {
                check(*a, true)?;
                check(*b, true)?;
            }
            Fixed(a) => check(*a, true)?,
            OnCurve(a, b) => {
                check(*a, true)?;
                check(*b, false)?;
            }
            Horizontal(a) | Vertical(a) | Length(a, _) | Radius(a, _) => check(*a, false)?,
            Parallel(a, b) | Perpendicular(a, b) | Tangent(a, b) | Equal(a, b) | Angle(a, b, _) => {
                check(*a, false)?;
                check(*b, false)?;
            }
        }
        values.push(match c {
            Length(_, v) | Distance(_, _, v) | DistanceX(_, _, v) | DistanceY(_, _, v) => {
                eval(v, vars)?
            }
            Radius(_, v) => eval(v, vars)?,
            Angle(_, _, v) => eval(v, vars)?.to_radians(),
            _ => 0.0,
        });
    }
    let fixed: Vec<usize> = d
        .constraints
        .iter()
        .filter_map(|c| if let Constraint::Fixed(p) = c { Some(*p) } else { None })
        .chain(pinned.iter().copied())
        .collect();
    let mut unknowns = Vec::new();
    for (k, [x, y]) in d.points.iter().enumerate() {
        if fixed.contains(&k) {
            continue;
        }
        if plain(x) {
            unknowns.push(Var::X(k));
        }
        if plain(y) {
            unknowns.push(Var::Y(k));
        }
    }
    for (k, c) in d.curves.iter().enumerate() {
        if matches!(c, Curve::Circle { radius, .. } if plain(radius)) {
            unknowns.push(Var::R(k));
        }
    }
    // Senses taken from the drawing as it is: which side a tangent circle touches,
    // which way a horizontal or vertical distance runs.
    let senses: Vec<f64> = d
        .constraints
        .iter()
        .map(|c| match c {
            Constraint::Tangent(a, b) => {
                let (pa, pb) = (prim(d, &g, *a), prim(d, &g, *b));
                match (pa, pb) {
                    (Prim::Arc { c: c1, r: r1, .. }, Prim::Arc { c: c2, r: r2, .. }) => {
                        let dd = dist(c1, c2);
                        if (dd - (r1 + r2)).abs() <= (dd - (r1 - r2).abs()).abs() {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    _ => 0.0,
                }
            }
            Constraint::DistanceX(a, b, _) => {
                if g.points[*b][0] >= g.points[*a][0] {
                    1.0
                } else {
                    -1.0
                }
            }
            Constraint::DistanceY(a, b, _) => {
                if g.points[*b][1] >= g.points[*a][1] {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => 0.0,
        })
        .collect();
    let residuals = |g: &Geometry| -> Vec<f64> {
        let mut r = Vec::new();
        let p = |i: usize| g.points[i];
        let line = |k: usize| match d.curves[k] {
            Curve::Line(a, b) | Curve::Guide(a, b) => Some((p(a), p(b))),
            _ => None,
        };
        let circle = |k: usize| match prim(d, g, k) {
            Prim::Arc { c, r, .. } => Some((c, r)),
            Prim::Seg(..) => None,
        };
        // An arc's end stays on its circle.
        for c in &d.curves {
            if let Curve::Arc { center, from, to } = *c {
                if from != to {
                    r.push(dist(p(to), p(center)) - dist(p(from), p(center)));
                }
            }
        }
        for (k, c) in d.constraints.iter().enumerate() {
            let v = values[k];
            use Constraint::*;
            match *c {
                Coincident(a, b) => {
                    r.push(p(a)[0] - p(b)[0]);
                    r.push(p(a)[1] - p(b)[1]);
                }
                OnCurve(a, b) => match (line(b), circle(b)) {
                    (Some((s, e)), _) => r.push(cross(unit(sub(e, s)), sub(p(a), s))),
                    (_, Some((c, rad))) => r.push(dist(p(a), c) - rad),
                    _ => {}
                },
                Horizontal(a) => {
                    if let Some((s, e)) = line(a) {
                        r.push(e[1] - s[1]);
                    }
                }
                Vertical(a) => {
                    if let Some((s, e)) = line(a) {
                        r.push(e[0] - s[0]);
                    }
                }
                Parallel(a, b) | Perpendicular(a, b) | Angle(a, b, _) => {
                    if let (Some((s1, e1)), Some((s2, e2))) = (line(a), line(b)) {
                        let (u, w) = (unit(sub(e1, s1)), unit(sub(e2, s2)));
                        r.push(match *c {
                            Parallel(..) => cross(u, w),
                            Perpendicular(..) => dot(u, w),
                            _ => cross(u, w) * v.cos() - dot(u, w) * v.sin(),
                        });
                    }
                }
                Tangent(a, b) => match (line(a), circle(a), line(b), circle(b)) {
                    (Some((s, e)), _, _, Some((c, rad))) | (_, Some((c, rad)), Some((s, e)), _) => {
                        r.push(cross(unit(sub(e, s)), sub(c, s)).abs() - rad);
                    }
                    (_, Some((c1, r1)), _, Some((c2, r2))) => {
                        let want = if senses[k] > 0.0 { r1 + r2 } else { (r1 - r2).abs() };
                        r.push(dist(c1, c2) - want);
                    }
                    _ => {}
                },
                Equal(a, b) => match (line(a), circle(a), line(b), circle(b)) {
                    (Some((s1, e1)), _, Some((s2, e2)), _) => r.push(dist(s1, e1) - dist(s2, e2)),
                    (_, Some((_, r1)), _, Some((_, r2))) => r.push(r1 - r2),
                    _ => {}
                },
                Fixed(_) => {}
                Length(a, _) => {
                    if let Some((s, e)) = line(a) {
                        r.push(dist(s, e) - v);
                    }
                }
                Distance(a, b, _) => r.push(dist(p(a), p(b)) - v),
                DistanceX(a, b, _) => r.push(senses[k] * (p(b)[0] - p(a)[0]) - v),
                DistanceY(a, b, _) => r.push(senses[k] * (p(b)[1] - p(a)[1]) - v),
                Radius(a, _) => {
                    if let Some((_, rad)) = circle(a) {
                        r.push(rad - v);
                    }
                }
            }
        }
        r
    };
    let get = |g: &Geometry, v: Var| match v {
        Var::X(k) => g.points[k][0],
        Var::Y(k) => g.points[k][1],
        Var::R(k) => g.radii[k],
    };
    let set = |g: &mut Geometry, v: Var, x: f64| match v {
        Var::X(k) => g.points[k][0] = x,
        Var::Y(k) => g.points[k][1] = x,
        Var::R(k) => g.radii[k] = x,
    };
    let norm = |r: &[f64]| r.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mut r = residuals(&g);
    let mut lambda = 1e-6;
    for _ in 0..200 {
        let now = norm(&r);
        if now < 1e-10 || unknowns.is_empty() {
            break;
        }
        // The Jacobian by central differences, one column per unknown.
        let (m, n) = (r.len(), unknowns.len());
        let mut jac = vec![0.0; m * n];
        for (j, &u) in unknowns.iter().enumerate() {
            let x0 = get(&g, u);
            let h = 1e-6 * x0.abs().max(1.0);
            set(&mut g, u, x0 + h);
            let hi = residuals(&g);
            set(&mut g, u, x0 - h);
            let lo = residuals(&g);
            set(&mut g, u, x0);
            for i in 0..m {
                jac[i * n + j] = (hi[i] - lo[i]) / (2.0 * h);
            }
        }
        // The least change that meets the constraints: dx = −Jᵀ (J Jᵀ + λI)⁻¹ r, so
        // whatever isn't held stays put.
        let mut improved = false;
        while lambda < 1e8 {
            let mut a = vec![0.0; m * m];
            for i in 0..m {
                for k in 0..m {
                    a[i * m + k] = (0..n).map(|j| jac[i * n + j] * jac[k * n + j]).sum();
                }
                a[i * m + i] += lambda;
            }
            let Some(y) = linear(&mut a, r.clone(), m) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial = g.clone();
            for (j, &u) in unknowns.iter().enumerate() {
                let dx: f64 = (0..m).map(|i| jac[i * n + j] * y[i]).sum();
                set(&mut trial, u, get(&g, u) - dx);
            }
            let tr = residuals(&trial);
            if norm(&tr) < now {
                g = trial;
                r = tr;
                lambda = (lambda * 0.1).max(1e-12);
                improved = true;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    Ok((g, norm(&r)))
}

/// Solves `a x = b` (`a` is `n` × `n`, row by row) by elimination with partial pivoting.
fn linear(a: &mut [f64], mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let piv =
            (col..n).max_by(|&i, &j| a[i * n + col].abs().total_cmp(&a[j * n + col].abs()))?;
        if a[piv * n + col].abs() < 1e-14 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(piv * n + k, col * n + k);
            }
            b.swap(piv, col);
        }
        for i in col + 1..n {
            let f = a[i * n + col] / a[col * n + col];
            if f != 0.0 {
                for k in col..n {
                    a[i * n + k] -= f * a[col * n + k];
                }
                b[i] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|k| a[i * n + k] * x[k]).sum();
        x[i] = (b[i] - s) / a[i * n + i];
    }
    Some(x)
}

/// Writes solved numbers back into the drawing's plain-number fields (expressions keep
/// their text).
pub fn store(d: &mut Drawing, g: &Geometry) {
    for (q, p) in d.points.iter_mut().zip(&g.points) {
        for (s, v) in q.iter_mut().zip(p) {
            if plain(s) && s.trim().parse::<f64>().ok().is_none_or(|old| (old - v).abs() > 1e-9) {
                *s = fmt(*v);
            }
        }
    }
    for (c, r) in d.curves.iter_mut().zip(&g.radii) {
        if let Curve::Circle { radius, .. } = c {
            if plain(radius) && radius.trim().parse::<f64>().ok() != Some(*r) {
                *radius = fmt(*r);
            }
        }
    }
}

// ---- Outlines ------------------------------------------------------------------------

/// The drawing as a graph of straight pieces: nodes (points at one place share one),
/// edges, and the whole circles apart.
struct Graph {
    nodes: Vec<P>,
    edges: Vec<(usize, usize)>,
    circles: Vec<Vec<P>>,
}

fn graph(d: &Drawing, g: &Geometry) -> Graph {
    // Points at one place are one node.
    let mut node_of = vec![usize::MAX; g.points.len()];
    let mut nodes: Vec<P> = Vec::new();
    for (k, p) in g.points.iter().enumerate() {
        node_of[k] = nodes.iter().position(|q| dist(*p, *q) < TOL).unwrap_or_else(|| {
            nodes.push(*p);
            nodes.len() - 1
        });
    }
    let mut edges = Vec::new();
    let mut circles = Vec::new();
    for (k, c) in d.curves.iter().enumerate() {
        match *c {
            Curve::Line(a, b) => {
                if node_of[a] != node_of[b] {
                    edges.push((node_of[a], node_of[b]));
                }
            }
            Curve::Guide(..) => {}
            Curve::Circle { .. } => {
                if g.radii[k] > TOL {
                    let mut pts = prim(d, g, k).points();
                    pts.pop();
                    circles.push(pts);
                }
            }
            Curve::Arc { from, to, .. } => {
                let pr = prim(d, g, k);
                if matches!(pr, Prim::Arc { r, .. } if r < TOL) {
                    continue;
                }
                let pts = pr.points();
                let mut prev = node_of[from];
                for q in &pts[1..pts.len() - 1] {
                    nodes.push(*q);
                    edges.push((prev, nodes.len() - 1));
                    prev = nodes.len() - 1;
                }
                if prev != node_of[to] {
                    edges.push((prev, node_of[to]));
                }
            }
        }
    }
    Graph { nodes, edges, circles }
}

fn area(l: &[P]) -> f64 {
    let n = l.len();
    (0..n).map(|k| cross(l[k], l[(k + 1) % n])).sum::<f64>() / 2.0
}

/// The drawing's closed outlines, largest first: each whole circle, and the outside of
/// each connected piece of lines and arcs once the lines that lead nowhere are left out.
pub fn loops(d: &Drawing, g: &Geometry) -> Vec<Vec<P>> {
    let Graph { nodes, mut edges, circles } = graph(d, g);
    // Leave out what doesn't close: edges ending where nothing else does.
    loop {
        let mut degree = vec![0usize; nodes.len()];
        for &(a, b) in &edges {
            degree[a] += 1;
            degree[b] += 1;
        }
        let before = edges.len();
        edges.retain(|&(a, b)| degree[a] > 1 && degree[b] > 1);
        if edges.len() == before {
            break;
        }
    }
    // Faces by walking half-edges, each turning as far right as it can: bounded faces
    // run counter-clockwise, and each piece's outside clockwise.
    let half: Vec<(usize, usize)> = edges.iter().flat_map(|&(a, b)| [(a, b), (b, a)]).collect();
    let mut out_of: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (h, &(a, _)) in half.iter().enumerate() {
        out_of[a].push(h);
    }
    let dir = |h: usize| angle_of(sub(nodes[half[h].1], nodes[half[h].0]));
    for list in &mut out_of {
        list.sort_by(|&x, &y| dir(x).total_cmp(&dir(y)));
    }
    let mut seen = vec![false; half.len()];
    let mut out: Vec<Vec<P>> = circles;
    for start in 0..half.len() {
        if seen[start] {
            continue;
        }
        let mut face = Vec::new();
        let mut h = start;
        while !seen[h] {
            seen[h] = true;
            face.push(nodes[half[h].0]);
            let v = half[h].1;
            let twin = h ^ 1;
            let list = &out_of[v];
            let i = list.iter().position(|&x| x == twin).expect("twin leaves v");
            h = list[(i + list.len() - 1) % list.len()];
        }
        if area(&face) < -1e-9 {
            face.reverse();
            out.push(face);
        }
    }
    out.sort_by(|a, b| area(b).abs().total_cmp(&area(a).abs()));
    out
}

/// The drawing's longest run of lines and arcs end to end, as points: a sweep's path.
pub fn path(d: &Drawing, g: &Geometry) -> Vec<P> {
    let Graph { nodes, edges, circles } = graph(d, g);
    if edges.is_empty() {
        return circles
            .into_iter()
            .next()
            .map(|mut c| {
                c.push(c[0]);
                c
            })
            .unwrap_or_default();
    }
    let mut next: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nodes.len()];
    for (k, &(a, b)) in edges.iter().enumerate() {
        next[a].push((b, k));
        next[b].push((a, k));
    }
    let walk = |from: usize| {
        let mut used = vec![false; edges.len()];
        let mut pts = vec![nodes[from]];
        let mut at = from;
        while let Some(&(to, k)) = next[at].iter().find(|(_, k)| !used[*k]) {
            used[k] = true;
            pts.push(nodes[to]);
            at = to;
        }
        pts
    };
    let ends: Vec<usize> = (0..nodes.len()).filter(|&n| next[n].len() == 1).collect();
    let starts = if ends.is_empty() { vec![edges[0].0] } else { ends };
    starts.into_iter().map(walk).max_by_key(Vec::len).unwrap_or_default()
}

/// The drawing's profile, solved with these parameters: its outlines, largest first.
pub fn profile(d: &Drawing, vars: Vars) -> Result<Vec<Vec<(f32, f32)>>, String> {
    let (g, miss) = solve(d, vars, &[])?;
    if miss > 1e-4 {
        return Err("the sketch's constraints can't all be met".into());
    }
    let l = loops(d, &g);
    if l.is_empty() {
        return Err("the sketch has no closed outline".into());
    }
    Ok(l.into_iter().map(|l| l.into_iter().map(|p| (p[0] as f32, p[1] as f32)).collect()).collect())
}

/// An outline of corners and bulges as a drawing: the same points, lines and arcs.
pub fn from_sketch(s: &Sketch, vars: Vars) -> Drawing {
    let mut d = Drawing::default();
    let n = s.points.len();
    d.points = s.points.iter().map(|q| [q.x.clone(), q.y.clone()]).collect();
    for k in 0..n {
        let (a, b) = (k, (k + 1) % n);
        let bulge = if s.points[k].bulge.trim().is_empty() {
            Ok(0.0)
        } else {
            eval(&s.points[k].bulge, vars)
        };
        let pa = eval(&s.points[a].x, vars).and_then(|x| Ok([x, eval(&s.points[a].y, vars)?]));
        let pb = eval(&s.points[b].x, vars).and_then(|x| Ok([x, eval(&s.points[b].y, vars)?]));
        match (bulge, pa, pb) {
            (Ok(bu), Ok(pa), Ok(pb)) if bu.abs() > 1e-9 && dist(pa, pb) > 1e-9 => {
                let chord = dist(pa, pb);
                let off = chord / 2.0 * (1.0 - bu * bu) / (2.0 * bu);
                let nrm = unit([-(pb[1] - pa[1]), pb[0] - pa[0]]);
                let c = add(mul(add(pa, pb), 0.5), mul(nrm, off));
                d.points.push([fmt(c[0]), fmt(c[1])]);
                let center = d.points.len() - 1;
                let (from, to) = if bu > 0.0 { (a, b) } else { (b, a) };
                d.curves.push(Curve::Arc { center, from, to });
            }
            _ => d.curves.push(Curve::Line(a, b)),
        }
    }
    d
}

// ---- Editing -------------------------------------------------------------------------

/// The points curve `c` runs through (its ends, and an arc's or circle's centre).
pub fn refs(c: &Curve) -> Vec<usize> {
    match *c {
        Curve::Line(a, b) | Curve::Guide(a, b) => vec![a, b],
        Curve::Arc { center, from, to } => vec![center, from, to],
        Curve::Circle { center, .. } => vec![center],
    }
}

/// The curves and points constraint `c` names.
fn names(c: &Constraint) -> (Vec<usize>, Vec<usize>) {
    use Constraint::*;
    match c {
        Coincident(a, b) | Distance(a, b, _) | DistanceX(a, b, _) | DistanceY(a, b, _) => {
            (vec![], vec![*a, *b])
        }
        Fixed(a) => (vec![], vec![*a]),
        OnCurve(a, b) => (vec![*b], vec![*a]),
        Horizontal(a) | Vertical(a) | Length(a, _) | Radius(a, _) => (vec![*a], vec![]),
        Parallel(a, b) | Perpendicular(a, b) | Tangent(a, b) | Equal(a, b) | Angle(a, b, _) => {
            (vec![*a, *b], vec![])
        }
    }
}

fn renumber(c: &mut Constraint, curve: &dyn Fn(usize) -> usize, point: &dyn Fn(usize) -> usize) {
    use Constraint::*;
    match c {
        Coincident(a, b) | Distance(a, b, _) | DistanceX(a, b, _) | DistanceY(a, b, _) => {
            *a = point(*a);
            *b = point(*b);
        }
        Fixed(a) => *a = point(*a),
        OnCurve(a, b) => {
            *a = point(*a);
            *b = curve(*b);
        }
        Horizontal(a) | Vertical(a) | Length(a, _) | Radius(a, _) => *a = curve(*a),
        Parallel(a, b) | Perpendicular(a, b) | Tangent(a, b) | Equal(a, b) | Angle(a, b, _) => {
            *a = curve(*a);
            *b = curve(*b);
        }
    }
}

/// A new point at `p`.
pub fn add_point(d: &mut Drawing, p: P) -> usize {
    d.points.push([fmt(p[0]), fmt(p[1])]);
    d.points.len() - 1
}

/// The point at `p`, if there is one.
pub fn point_at(g: &Geometry, p: P) -> Option<usize> {
    g.points.iter().position(|q| dist(*q, p) < TOL * 10.0)
}

/// Removes these curves, the constraints on them, and the points nothing uses any more.
pub fn remove_curves(d: &mut Drawing, gone: &[usize]) {
    let n = d.curves.len();
    let map: Vec<Option<usize>> = {
        let mut next = 0;
        (0..n)
            .map(|k| {
                (!gone.contains(&k)).then(|| {
                    next += 1;
                    next - 1
                })
            })
            .collect()
    };
    d.constraints.retain(|c| names(c).0.iter().all(|k| map[*k].is_some()));
    for c in &mut d.constraints {
        renumber(c, &|k| map[k].expect("kept"), &|p| p);
    }
    let mut k = 0;
    d.curves.retain(|_| {
        k += 1;
        map[k - 1].is_some()
    });
    tidy(d);
}

/// Removes the points no curve or constraint uses.
pub fn tidy(d: &mut Drawing) {
    let mut used = vec![false; d.points.len()];
    for c in &d.curves {
        refs(c).into_iter().for_each(|p| used[p] = true);
    }
    for c in &d.constraints {
        names(c).1.into_iter().for_each(|p| used[p] = true);
    }
    let mut map = vec![usize::MAX; used.len()];
    let mut next = 0;
    for (k, u) in used.iter().enumerate() {
        if *u {
            map[k] = next;
            next += 1;
        }
    }
    let mut k = 0;
    d.points.retain(|_| {
        k += 1;
        used[k - 1]
    });
    let point = |p: usize| map[p];
    for c in &mut d.curves {
        match c {
            Curve::Line(a, b) | Curve::Guide(a, b) => {
                *a = point(*a);
                *b = point(*b);
            }
            Curve::Arc { center, from, to } => {
                *center = point(*center);
                *from = point(*from);
                *to = point(*to);
            }
            Curve::Circle { center, .. } => *center = point(*center),
        }
    }
    for c in &mut d.constraints {
        renumber(c, &|k| k, &point);
    }
}

/// Drops the constraints that name curve `k` and no longer hold once it's cut short.
fn loosen(d: &mut Drawing, k: usize) {
    use Constraint::*;
    d.constraints.retain(|c| {
        !names(c).0.contains(&k)
            || matches!(c, Horizontal(_) | Vertical(_) | Parallel(..) | Perpendicular(..))
    });
}

/// Splits curve `k` at point `p` (on it, between its ends), so what meets it there
/// joins it.
pub fn split(d: &mut Drawing, g: &Geometry, k: usize, p: usize) {
    let pr = prim(d, g, k);
    let t = pr.param(g.points[p]);
    let interior = pr.closed() || (t > 1e-9 && t < pr.end() - 1e-9);
    if refs(&d.curves[k]).contains(&p) || !interior {
        return;
    }
    let second = match d.curves[k].clone() {
        Curve::Line(a, b) => {
            d.curves[k] = Curve::Line(a, p);
            Curve::Line(p, b)
        }
        Curve::Guide(a, b) => {
            d.curves[k] = Curve::Guide(a, p);
            Curve::Guide(p, b)
        }
        Curve::Arc { center, from, to } => {
            d.curves[k] = Curve::Arc { center, from, to: p };
            Curve::Arc { center, from: p, to }
        }
        Curve::Circle { center, .. } => {
            // A circle with one point on it: a whole arc from there.
            d.curves[k] = Curve::Arc { center, from: p, to: p };
            loosen(d, k);
            return;
        }
    };
    loosen(d, k);
    d.curves.push(second);
}

/// The point at `x` (an existing one there, or a new one), splitting the curves it lies
/// within (but `skip`) so they join there.
pub fn join_at(d: &mut Drawing, g: &mut Geometry, x: P, skip: Option<usize>) -> usize {
    let p = point_at(g, x).unwrap_or_else(|| {
        g.points.push(x);
        add_point(d, x)
    });
    for k in 0..d.curves.len() {
        if Some(k) == skip || matches!(d.curves[k], Curve::Guide(..)) {
            continue;
        }
        if dist(prim(d, g, k).nearest(x), x) < TOL * 10.0 {
            split(d, g, k, p);
            g.radii.resize(d.curves.len(), 0.0);
        }
    }
    p
}

/// Trims curve `k` back to the curves that cross it, taking away the piece nearest `at`;
/// a curve that nothing crosses is removed whole. True when something changed.
pub fn trim(d: &mut Drawing, g: &Geometry, k: usize, at: P) -> bool {
    let pk = prim(d, g, k);
    let tq = pk.param(at);
    let mut cuts: Vec<f64> = (0..d.curves.len())
        .filter(|&j| j != k)
        .flat_map(|j| hits(&pk, &prim(d, g, j), false))
        .map(|(t, _)| t)
        .filter(|&t| pk.closed() || (t > 1e-7 && t < pk.end() - 1e-7))
        .collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
    if cuts.is_empty() || (pk.closed() && cuts.len() < 2) {
        remove_curves(d, &[k]);
        return true;
    }
    let below = cuts.iter().rev().find(|&&t| t < tq).copied();
    let above = cuts.iter().find(|&&t| t > tq).copied();
    let (lo, hi) = if pk.closed() {
        (below.or(cuts.last().copied()), above.or(cuts.first().copied()))
    } else {
        (below, above)
    };
    let mut g = g.clone();
    let plo = lo.map(|t| join_at(d, &mut g, pk.at(t), Some(k)));
    let phi = hi.map(|t| join_at(d, &mut g, pk.at(t), Some(k)));
    let mut pieces = Vec::new();
    match d.curves[k].clone() {
        Curve::Line(a, b) | Curve::Guide(a, b) => {
            let guide = matches!(d.curves[k], Curve::Guide(..));
            let mk = |x, y| if guide { Curve::Guide(x, y) } else { Curve::Line(x, y) };
            pieces.extend(plo.map(|p| mk(a, p)));
            pieces.extend(phi.map(|p| mk(p, b)));
        }
        Curve::Arc { center, .. } | Curve::Circle { center, .. } if pk.closed() => {
            if let (Some(l), Some(h)) = (plo, phi) {
                pieces.push(Curve::Arc { center, from: h, to: l });
            }
        }
        Curve::Arc { center, from, to } => {
            pieces.extend(plo.map(|p| Curve::Arc { center, from, to: p }));
            pieces.extend(phi.map(|p| Curve::Arc { center, from: p, to }));
        }
        Curve::Circle { .. } => {}
    }
    d.curves.extend(pieces);
    remove_curves(d, &[k]);
    true
}

/// Extends line `k`, at its end nearer `at`, to the first curve it meets. True when it
/// reached one.
pub fn extend(d: &mut Drawing, g: &Geometry, k: usize, at: P) -> bool {
    let (Curve::Line(a, b) | Curve::Guide(a, b)) = d.curves[k] else { return false };
    let pk = prim(d, g, k);
    let from_end = dist(at, g.points[b]) < dist(at, g.points[a]);
    let ts = (0..d.curves.len())
        .filter(|&j| j != k)
        .flat_map(|j| hits(&pk, &prim(d, g, j), true))
        .map(|(t, _)| t);
    let t = if from_end {
        ts.filter(|&t| t > 1.0 + 1e-7).min_by(f64::total_cmp)
    } else {
        ts.filter(|&t| t < -1e-7).max_by(f64::total_cmp)
    };
    let Some(t) = t else { return false };
    let mut g = g.clone();
    let x = pk.at(t);
    let p = join_at(d, &mut g, x, Some(k));
    d.curves[k] =
        if from_end { line_like(&d.curves[k], a, p) } else { line_like(&d.curves[k], p, b) };
    tidy(d);
    true
}

fn line_like(c: &Curve, a: usize, b: usize) -> Curve {
    if matches!(c, Curve::Guide(..)) { Curve::Guide(a, b) } else { Curve::Line(a, b) }
}

/// The run of curves through curve `k`, end to end where only two meet: each with
/// whether it runs backwards, and whether the run closes.
fn run(d: &Drawing, g: &Geometry, k: usize) -> (Vec<(usize, bool)>, bool) {
    let ends = |j: usize| -> Option<(usize, usize)> {
        match d.curves[j] {
            Curve::Line(a, b) => Some((a, b)),
            Curve::Arc { from, to, .. } if from != to => Some((from, to)),
            _ => None,
        }
    };
    let same = |p: usize, q: usize| p == q || dist(g.points[p], g.points[q]) < TOL;
    let Some(_) = ends(k) else { return (vec![(k, false)], false) };
    let meeting = |p: usize| -> Vec<usize> {
        (0..d.curves.len())
            .filter(|&j| ends(j).is_some_and(|(a, b)| same(a, p) || same(b, p)))
            .collect()
    };
    let mut out = vec![(k, false)];
    // Forwards from k's end.
    let mut closed = false;
    loop {
        let &(j, rev) = out.last().expect("one");
        let (a, b) = ends(j).expect("ends");
        let tip = if rev { a } else { b };
        let m = meeting(tip);
        if m.len() != 2 {
            break;
        }
        let next = if m[0] == j { m[1] } else { m[0] };
        if next == k {
            closed = true;
            break;
        }
        if out.iter().any(|(x, _)| *x == next) {
            break;
        }
        let (na, _) = ends(next).expect("ends");
        out.push((next, !same(na, tip)));
    }
    if !closed {
        loop {
            let &(j, rev) = out.first().expect("one");
            let (a, b) = ends(j).expect("ends");
            let tail = if rev { b } else { a };
            let m = meeting(tail);
            if m.len() != 2 {
                break;
            }
            let prev = if m[0] == j { m[1] } else { m[0] };
            if out.iter().any(|(x, _)| *x == prev) {
                break;
            }
            let (_, nb) = ends(prev).expect("ends");
            out.insert(0, (prev, !same(nb, tail)));
        }
    }
    (out, closed)
}

/// An offset piece: a line, or an arc about centre point `center` with this radius,
/// running from one end to the other (backwards for an arc that runs clockwise).
#[derive(Debug, Clone, Copy)]
enum Piece {
    Line(P, P),
    Arc { center: usize, c: P, r: f64, rev: bool, s: P, e: P },
}

/// Copies curve `k` (and the run of curves it is part of) `distance` away, on the side
/// `toward` is. True when something was made.
pub fn offset(d: &mut Drawing, g: &Geometry, k: usize, distance: f64, toward: P) -> bool {
    if distance <= 0.0 {
        return false;
    }
    if let Curve::Circle { center, .. } = d.curves[k] {
        let r = g.radii[k];
        let c = g.points[center];
        let r2 = if dist(toward, c) < r { r - distance } else { r + distance };
        if r2 <= TOL {
            return false;
        }
        d.curves.push(Curve::Circle { center, radius: fmt(r2) });
        return true;
    }
    if matches!(d.curves[k], Curve::Arc { from, to, .. } if from == to) {
        return false;
    }
    let (chain, closed) = run(d, g, k);
    // Which side: left (+) or right (−) of curve k as the run goes.
    let pk = prim(d, g, k);
    let rev_k = chain.iter().find(|(j, _)| *j == k).is_some_and(|x| x.1);
    let t = pk.param(toward);
    let tangent = match pk {
        Prim::Seg(a, b) => sub(b, a),
        Prim::Arc { c, .. } => {
            let q = sub(pk.at(t), c);
            [-q[1], q[0]]
        }
    };
    let tangent = if rev_k { mul(tangent, -1.0) } else { tangent };
    let side = if cross(tangent, sub(toward, pk.at(t))) >= 0.0 { 1.0 } else { -1.0 };
    let off = side * distance;
    let mut pieces = Vec::new();
    for &(j, rev) in &chain {
        let pj = prim(d, g, j);
        match (pj, &d.curves[j]) {
            (Prim::Seg(a, b), _) => {
                let (a, b) = if rev { (b, a) } else { (a, b) };
                let n = unit([-(b[1] - a[1]), b[0] - a[0]]);
                pieces.push(Piece::Line(add(a, mul(n, off)), add(b, mul(n, off))));
            }
            (Prim::Arc { c, r, .. }, Curve::Arc { center, .. }) => {
                // Left of a counter-clockwise arc is towards its centre.
                let r2 = if rev { r + off } else { r - off };
                if r2 <= TOL {
                    return false;
                }
                let along = |q: P| add(c, mul(unit(sub(q, c)), r2));
                let (s, e) = (along(pj.at(0.0)), along(pj.at(pj.end())));
                let (s, e) = if rev { (e, s) } else { (s, e) };
                pieces.push(Piece::Arc { center: *center, c, r: r2, rev, s, e });
            }
            _ => return false,
        }
    }
    // Where neighbouring pieces meet: their crossing nearest the corner they offset.
    let n = pieces.len();
    let start = |p: &Piece| match *p {
        Piece::Line(a, _) => a,
        Piece::Arc { s, .. } => s,
    };
    let end = |p: &Piece| match *p {
        Piece::Line(_, b) => b,
        Piece::Arc { e, .. } => e,
    };
    let as_prim = |p: &Piece| match *p {
        Piece::Line(a, b) => Prim::Seg(a, b),
        Piece::Arc { c, r, .. } => Prim::Arc { c, r, a0: 0.0, sweep: std::f64::consts::TAU },
    };
    let joints: Vec<P> = (0..n)
        .map(|i| {
            let (p, q) = (&pieces[i], &pieces[(i + 1) % n]);
            let near = mul(add(end(p), start(q)), 0.5);
            if dist(end(p), start(q)) < TOL {
                return near;
            }
            let found = hits(&as_prim(p), &as_prim(q), true);
            let pp = as_prim(p);
            found
                .into_iter()
                .map(|(t, _)| pp.at(t))
                .min_by(|x, y| dist(*x, near).total_cmp(&dist(*y, near)))
                .unwrap_or(near)
        })
        .collect();
    let ids: Vec<usize> = joints.iter().map(|j| add_point(d, *j)).collect();
    let (first, last) = if closed {
        (None, None)
    } else {
        (Some(add_point(d, start(&pieces[0]))), Some(add_point(d, end(&pieces[n - 1]))))
    };
    for (i, p) in pieces.iter().enumerate() {
        let s = if i == 0 { first.unwrap_or(ids[n - 1]) } else { ids[i - 1] };
        let e = if i == n - 1 { last.unwrap_or(ids[n - 1]) } else { ids[i] };
        d.curves.push(match *p {
            Piece::Line(..) => Curve::Line(s, e),
            Piece::Arc { center, rev, .. } => {
                if rev {
                    Curve::Arc { center, from: e, to: s }
                } else {
                    Curve::Arc { center, from: s, to: e }
                }
            }
        });
    }
    true
}

/// Mirrored copies of these curves across line `axis`; points on the axis are shared.
pub fn mirror(d: &mut Drawing, g: &Geometry, curves: &[usize], axis: usize) -> bool {
    let (Curve::Line(a, b) | Curve::Guide(a, b)) = d.curves[axis] else { return false };
    let (o, u) = (g.points[a], unit(sub(g.points[b], g.points[a])));
    let flip = |p: P| {
        let v = sub(p, o);
        let along = mul(u, dot(v, u));
        add(o, sub(mul(along, 2.0), v))
    };
    let mut map: Vec<Option<usize>> = vec![None; d.points.len()];
    let mut image = |d: &mut Drawing, p: usize| -> usize {
        if let Some(q) = map[p] {
            return q;
        }
        let m = flip(g.points[p]);
        let q = if dist(m, g.points[p]) < TOL { p } else { add_point(d, m) };
        map[p] = Some(q);
        q
    };
    let mut made = false;
    for &k in curves.iter().filter(|&&k| k != axis) {
        let c = match d.curves[k].clone() {
            Curve::Line(p, q) => Curve::Line(image(d, p), image(d, q)),
            Curve::Guide(p, q) => Curve::Guide(image(d, p), image(d, q)),
            // A reflection turns the arc's direction round.
            Curve::Arc { center, from, to } => {
                Curve::Arc { center: image(d, center), from: image(d, to), to: image(d, from) }
            }
            Curve::Circle { center, radius } => Curve::Circle { center: image(d, center), radius },
        };
        d.curves.push(c);
        made = true;
    }
    made
}

/// The two lines that meet at point `p`, with their other ends.
fn corner(d: &Drawing, p: usize) -> Option<[(usize, usize); 2]> {
    let lines: Vec<(usize, usize)> = d
        .curves
        .iter()
        .enumerate()
        .filter_map(|(k, c)| match *c {
            Curve::Line(a, b) if a == p => Some((k, b)),
            Curve::Line(a, b) if b == p => Some((k, a)),
            _ => None,
        })
        .collect();
    (lines.len() == 2).then(|| [lines[0], lines[1]])
}

/// Moves line `k`'s end at `p` to `q`.
fn retarget(d: &mut Drawing, k: usize, p: usize, q: usize) {
    if let Curve::Line(a, b) = &mut d.curves[k] {
        if *a == p {
            *a = q;
        } else if *b == p {
            *b = q;
        }
    }
}

/// Rounds the corner where two lines meet at point `p` with an arc of radius `r`,
/// tangent to both. True when it fits.
pub fn fillet(d: &mut Drawing, g: &Geometry, p: usize, r: f64) -> bool {
    let Some([(k1, a), (k2, b)]) = corner(d, p) else { return false };
    let o = g.points[p];
    let (u, v) = (unit(sub(g.points[a], o)), unit(sub(g.points[b], o)));
    let theta = dot(u, v).clamp(-1.0, 1.0).acos();
    if r <= 0.0 || theta < 1e-6 || theta > std::f64::consts::PI - 1e-6 {
        return false;
    }
    let t = r / (theta / 2.0).tan();
    if t > dist(g.points[a], o) + 1e-9 || t > dist(g.points[b], o) + 1e-9 {
        return false;
    }
    let (p1, p2) = (add(o, mul(u, t)), add(o, mul(v, t)));
    let c = add(o, mul(unit(add(u, v)), r / (theta / 2.0).sin()));
    let (i1, i2, ic) = (add_point(d, p1), add_point(d, p2), add_point(d, c));
    retarget(d, k1, p, i1);
    retarget(d, k2, p, i2);
    let arc = if cross(sub(p1, c), sub(p2, c)) > 0.0 {
        Curve::Arc { center: ic, from: i1, to: i2 }
    } else {
        Curve::Arc { center: ic, from: i2, to: i1 }
    };
    d.curves.push(arc);
    let k = d.curves.len() - 1;
    d.constraints.retain(|c| !matches!(c, Constraint::Coincident(x, y) if *x == p || *y == p));
    d.constraints.push(Constraint::Tangent(k1, k));
    d.constraints.push(Constraint::Tangent(k2, k));
    tidy(d);
    true
}

/// Bevels the corner where two lines meet at point `p`, cutting `distance` back along
/// each. True when it fits.
pub fn chamfer(d: &mut Drawing, g: &Geometry, p: usize, distance: f64) -> bool {
    let Some([(k1, a), (k2, b)]) = corner(d, p) else { return false };
    let o = g.points[p];
    if distance <= 0.0
        || distance > dist(g.points[a], o) + 1e-9
        || distance > dist(g.points[b], o) + 1e-9
    {
        return false;
    }
    let p1 = add(o, mul(unit(sub(g.points[a], o)), distance));
    let p2 = add(o, mul(unit(sub(g.points[b], o)), distance));
    let (i1, i2) = (add_point(d, p1), add_point(d, p2));
    retarget(d, k1, p, i1);
    retarget(d, k2, p, i2);
    d.curves.push(Curve::Line(i1, i2));
    tidy(d);
    true
}

/// The centre of the circle through three points, if they aren't in a line.
pub fn circle_through(a: P, b: P, c: P) -> Option<P> {
    let den = 2.0 * cross(sub(b, a), sub(c, a));
    if den.abs() < 1e-12 {
        return None;
    }
    let (ab, ac) = (sub(b, a), sub(c, a));
    let (l1, l2) = (dot(ab, ab), dot(ac, ac));
    Some(add(a, [(ac[1] * l1 - ab[1] * l2) / den, (ab[0] * l2 - ac[0] * l1) / den]))
}

// ---- Snaps ---------------------------------------------------------------------------

/// What a snap caught.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    End,
    Mid,
    Center,
    Intersection,
    Nearest,
    Grid,
}

impl Snap {
    pub fn label(self) -> &'static str {
        match self {
            Snap::End => "endpoint",
            Snap::Mid => "midpoint",
            Snap::Center => "centre",
            Snap::Intersection => "intersection",
            Snap::Nearest => "nearest",
            Snap::Grid => "grid",
        }
    }
}

/// The places worth snapping to: ends, middles, centres and crossings.
pub fn snap_points(d: &Drawing, g: &Geometry) -> Vec<(P, Snap)> {
    let mut out = Vec::new();
    let prims: Vec<Prim> = (0..d.curves.len()).map(|k| prim(d, g, k)).collect();
    for (k, c) in d.curves.iter().enumerate() {
        let pr = prims[k];
        match *c {
            Curve::Line(a, b) | Curve::Guide(a, b) => {
                out.push((g.points[a], Snap::End));
                out.push((g.points[b], Snap::End));
                out.push((pr.at(0.5), Snap::Mid));
            }
            Curve::Arc { center, from, to } => {
                out.push((g.points[from], Snap::End));
                out.push((g.points[to], Snap::End));
                out.push((g.points[center], Snap::Center));
                out.push((pr.at(pr.end() / 2.0), Snap::Mid));
            }
            Curve::Circle { center, .. } => out.push((g.points[center], Snap::Center)),
        }
    }
    for i in 0..prims.len() {
        for j in i + 1..prims.len() {
            for (t, _) in hits(&prims[i], &prims[j], false) {
                out.push((prims[i].at(t), Snap::Intersection));
            }
        }
    }
    out
}

/// The curve nearest `q` within `reach`, and the nearest point on it.
pub fn nearest_curve(d: &Drawing, g: &Geometry, q: P, reach: f64) -> Option<(usize, P)> {
    (0..d.curves.len())
        .map(|k| (k, prim(d, g, k).nearest(q)))
        .filter(|(_, p)| dist(*p, q) <= reach)
        .min_by(|a, b| dist(a.1, q).total_cmp(&dist(b.1, q)))
}

/// The nearest point within `reach`.
pub fn nearest_point(g: &Geometry, q: P, reach: f64) -> Option<usize> {
    (0..g.points.len())
        .filter(|&k| dist(g.points[k], q) <= reach)
        .min_by(|&a, &b| dist(g.points[a], q).total_cmp(&dist(g.points[b], q)))
}

// ---- Typed input ---------------------------------------------------------------------

/// A point typed as AutoCAD takes them: `x,y`; `@dx,dy` from the last point; `@d<a`
/// (or `d<a` from the origin) by length and angle in degrees; or a bare length along
/// `toward`'s direction from the last point. Numbers may be expressions.
pub fn typed(text: &str, last: Option<P>, toward: P, vars: Vars) -> Result<P, String> {
    let t = text.trim();
    let num = |s: &str| eval(s, vars);
    let (rel, body) = match t.strip_prefix('@') {
        Some(b) => (true, b),
        None => (false, t),
    };
    let base = if rel { last.unwrap_or([0.0, 0.0]) } else { [0.0, 0.0] };
    if let Some((l, a)) = body.split_once('<') {
        let (l, a) = (num(l)?, num(a)?.to_radians());
        return Ok(add(base, [l * a.cos(), l * a.sin()]));
    }
    if let Some((x, y)) = body.split_once(',') {
        return Ok(add(base, [num(x)?, num(y)?]));
    }
    let l = num(body)?;
    let from = last.ok_or("type a point as x,y first")?;
    let dir = sub(toward, from);
    let dir = if len(dir) < 1e-9 { [1.0, 0.0] } else { unit(dir) };
    Ok(add(from, mul(dir, l)))
}

/// `to` held to a horizontal or vertical line from `from` (ortho), or, with `polar`
/// degrees, to the nearest multiple of that angle when within a few degrees of it.
pub fn track(from: P, to: P, ortho: bool, polar: Option<f64>) -> (P, bool) {
    let v = sub(to, from);
    let l = len(v);
    if l < 1e-9 {
        return (to, false);
    }
    let a = angle_of(v);
    if ortho {
        let step = std::f64::consts::FRAC_PI_2;
        let s = (a / step).round() * step;
        let along = dot(v, [s.cos(), s.sin()]);
        return (add(from, [along * s.cos(), along * s.sin()]), true);
    }
    if let Some(deg) = polar.filter(|d| *d > 0.0) {
        let step = deg.to_radians();
        let s = (a / step).round() * step;
        if (a - s).abs() < 3f64.to_radians() {
            let along = dot(v, [s.cos(), s.sin()]);
            return (add(from, [along * s.cos(), along * s.sin()]), true);
        }
    }
    (to, false)
}

/// A `w` × `h` rectangle from corner `a` to corner `b`: four lines, held square.
pub fn rectangle(d: &mut Drawing, a: P, b: P) {
    let p = [a, [b[0], a[1]], b, [a[0], b[1]]].map(|q| add_point(d, q));
    let k = d.curves.len();
    for i in 0..4 {
        d.curves.push(Curve::Line(p[i], p[(i + 1) % 4]));
    }
    d.constraints.extend([
        Constraint::Horizontal(k),
        Constraint::Vertical(k + 1),
        Constraint::Horizontal(k + 2),
        Constraint::Vertical(k + 3),
    ]);
}

/// A regular polygon of `n` sides about `c`, one corner at `at`: lines of equal length.
pub fn polygon(d: &mut Drawing, c: P, at: P, n: usize) {
    let n = n.max(3);
    let (r, a0) = (dist(at, c), angle_of(sub(at, c)));
    let pts: Vec<usize> = (0..n)
        .map(|k| {
            let a = a0 + std::f64::consts::TAU * k as f64 / n as f64;
            add_point(d, [c[0] + r * a.cos(), c[1] + r * a.sin()])
        })
        .collect();
    let k = d.curves.len();
    for i in 0..n {
        d.curves.push(Curve::Line(pts[i], pts[(i + 1) % n]));
    }
    for i in 1..n {
        d.constraints.push(Constraint::Equal(k, k + i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<f64> {
        None
    }

    fn area_of(l: &[P]) -> f64 {
        area(l).abs()
    }

    #[test]
    fn constraints_square_up_a_rough_rectangle_to_its_dimensions() {
        let mut d = Drawing::default();
        rectangle(&mut d, [0.0, 0.0], [10.0, 5.0]);
        // Knocked out of square; then sized by dimensions, one of them a parameter.
        d.points[2] = ["11".into(), "6".into()];
        d.constraints.push(Constraint::Fixed(0));
        d.constraints.push(Constraint::Length(0, "w".into()));
        d.constraints.push(Constraint::Length(1, "w/2".into()));
        let vars = |n: &str| (n == "w").then_some(20.0);
        let (g, miss) = solve(&d, &vars, &[]).unwrap();
        assert!(miss < 1e-8, "{miss}");
        let close = |p: P, q: P| dist(p, q) < 1e-6;
        assert!(close(g.points[0], [0.0, 0.0]), "{:?}", g.points);
        assert!(close(g.points[2], [20.0, 10.0]), "{:?}", g.points);
        let l = loops(&d, &g);
        assert_eq!(l.len(), 1);
        assert!((area_of(&l[0]) - 200.0).abs() < 1e-6);
        // More than can be met is reported.
        d.constraints.push(Constraint::Distance(0, 2, "5".into()));
        assert!(solve(&d, &vars, &[]).unwrap().1 > 1e-3);
    }

    #[test]
    fn tangents_angles_and_points_on_curves() {
        let mut d = Drawing::default();
        let c = add_point(&mut d, [0.0, 0.0]);
        d.curves.push(Curve::Circle { center: c, radius: "3".into() });
        let (a, b) = (add_point(&mut d, [-10.0, 5.0]), add_point(&mut d, [10.0, 4.0]));
        d.curves.push(Curve::Line(a, b));
        let e = add_point(&mut d, [10.0, -10.0]);
        d.curves.push(Curve::Line(a, e));
        d.constraints.extend([
            Constraint::Fixed(c),
            Constraint::Radius(0, "5".into()),
            Constraint::Tangent(1, 0),
            Constraint::Horizontal(1),
            Constraint::Angle(1, 2, "-30".into()),
        ]);
        let (g, miss) = solve(&d, &none, &[]).unwrap();
        assert!(miss < 1e-8, "{miss}");
        assert!((g.radii[0] - 5.0).abs() < 1e-8);
        assert!((g.points[a][1].abs() - 5.0).abs() < 1e-6, "{:?}", g.points);
        let dir = sub(g.points[e], g.points[a]);
        assert!((angle_of(dir).to_degrees() + 30.0).abs() < 1e-5);
        let mut d2 = d.clone();
        let q = add_point(&mut d2, [1.0, 1.0]);
        d2.constraints.push(Constraint::OnCurve(q, 0));
        let (g2, miss) = solve(&d2, &none, &[]).unwrap();
        assert!(miss < 1e-8 && (len(g2.points[q]) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn trimming_overlapping_squares_leaves_their_union() {
        let mut d = Drawing::default();
        rectangle(&mut d, [0.0, 0.0], [10.0, 10.0]);
        rectangle(&mut d, [5.0, 5.0], [15.0, 15.0]);
        d.constraints.clear();
        let g = geometry(&d, &none).unwrap();
        // Unjoined they are two outlines.
        assert_eq!(loops(&d, &g).len(), 2);
        // Trim the first square's top and right inside the second, and the second's
        // left and bottom inside the first.
        for at in [[7.5, 10.0], [10.0, 7.5], [5.0, 7.5], [7.5, 5.0]] {
            let g = geometry(&d, &none).unwrap();
            let (k, _) = nearest_curve(&d, &g, at, 0.1).unwrap();
            assert!(trim(&mut d, &g, k, at));
        }
        let g = geometry(&d, &none).unwrap();
        let l = loops(&d, &g);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!((area_of(&l[0]) - 175.0).abs() < 1e-6, "{}", area_of(&l[0]));
        // A line nothing crosses goes whole.
        let (a, b) = (add_point(&mut d, [20.0, 0.0]), add_point(&mut d, [30.0, 0.0]));
        d.curves.push(Curve::Line(a, b));
        let g = geometry(&d, &none).unwrap();
        let n = d.curves.len();
        assert!(trim(&mut d, &g, n - 1, [25.0, 0.0]));
        assert_eq!(d.curves.len(), n - 1);
    }

    #[test]
    fn extend_offset_mirror_fillet_and_chamfer() {
        // A line extended to meet another.
        let mut d = Drawing::default();
        let p = [[0.0, 0.0], [5.0, 0.0], [10.0, -5.0], [10.0, 5.0]].map(|q| add_point(&mut d, q));
        d.curves.push(Curve::Line(p[0], p[1]));
        d.curves.push(Curve::Line(p[2], p[3]));
        let g = geometry(&d, &none).unwrap();
        assert!(extend(&mut d, &g, 0, [4.0, 0.0]));
        let g = geometry(&d, &none).unwrap();
        assert!(matches!(prim(&d, &g, 0), Prim::Seg(_, b) if dist(b, [10.0, 0.0]) < 1e-9));
        assert_eq!(d.curves.len(), 3, "the line met is split there");

        // A rounded square offset inwards by 1: its corners stay round.
        let mut d = Drawing::default();
        rectangle(&mut d, [0.0, 0.0], [10.0, 10.0]);
        for corner in 0..4 {
            let g = geometry(&d, &none).unwrap();
            let at = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]][corner];
            let p = point_at(&g, at).unwrap();
            assert!(fillet(&mut d, &g, p, 2.0));
        }
        let g = geometry(&d, &none).unwrap();
        let full = area_of(&loops(&d, &g)[0]);
        let want = 100.0 - 4.0 * (4.0 - std::f64::consts::PI);
        assert!((full - want).abs() < 0.1, "{full} vs {want}");
        let (g, miss) = solve(&d, &none, &[]).unwrap();
        assert!(miss < 1e-8, "the fillets' tangents hold: {miss}");
        let k = nearest_curve(&d, &g, [5.0, 0.0], 0.1).unwrap().0;
        assert!(offset(&mut d, &g, k, 1.0, [5.0, 1.0]));
        let g = geometry(&d, &none).unwrap();
        let l = loops(&d, &g);
        assert_eq!(l.len(), 2);
        let inner = area_of(&l[1]);
        let want = 64.0 - 4.0 * (1.0 - std::f64::consts::PI / 4.0);
        assert!((inner - want).abs() < 0.1, "{inner} vs {want}");

        // Half a shape mirrored across a guide closes it.
        let mut d = Drawing::default();
        let p = [[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]].map(|q| add_point(&mut d, q));
        d.curves.push(Curve::Guide(p[0], p[3]));
        d.curves.push(Curve::Line(p[0], p[1]));
        d.curves.push(Curve::Line(p[1], p[2]));
        d.curves.push(Curve::Line(p[2], p[3]));
        let g = geometry(&d, &none).unwrap();
        assert!(loops(&d, &g).is_empty());
        assert!(mirror(&mut d, &g, &[1, 2, 3], 0));
        let g = geometry(&d, &none).unwrap();
        let l = loops(&d, &g);
        assert_eq!(l.len(), 1);
        assert!((area_of(&l[0]) - 50.0).abs() < 1e-9);

        // A chamfered corner.
        let mut d = Drawing::default();
        rectangle(&mut d, [0.0, 0.0], [10.0, 10.0]);
        let g = geometry(&d, &none).unwrap();
        assert!(chamfer(&mut d, &g, 0, 2.0));
        let g = geometry(&d, &none).unwrap();
        assert!((area_of(&loops(&d, &g)[0]) - 98.0).abs() < 1e-9);
    }

    #[test]
    fn typed_points_tracking_and_old_sketches() {
        let none = |_: &str| None;
        let last = Some([10.0, 10.0]);
        let close = |p: P, q: P| dist(p, q) < 1e-9;
        assert!(close(typed("5,2", last, [0.0; 2], &none).unwrap(), [5.0, 2.0]));
        assert!(close(typed("@5,-2", last, [0.0; 2], &none).unwrap(), [15.0, 8.0]));
        assert!(close(typed("@10<90", last, [0.0; 2], &none).unwrap(), [10.0, 20.0]));
        assert!(close(typed("4", last, [20.0, 10.0], &none).unwrap(), [14.0, 10.0]));
        let w = |n: &str| (n == "w").then_some(8.0);
        assert!(close(typed("@w/2,0", last, [0.0; 2], &w).unwrap(), [14.0, 10.0]));
        assert!(typed("@1<", last, [0.0; 2], &none).is_err());
        let (p, held) = track([0.0, 0.0], [10.0, 1.0], true, None);
        assert!(held && close(p, [10.0, 0.0]));
        let (p, held) = track([0.0, 0.0], [10.0, 10.3], false, Some(45.0));
        assert!(held && (p[0] - p[1]).abs() < 1e-9);
        // A slot of lines and half circles becomes lines and arcs with centres.
        let d = from_sketch(&Sketch::slot(10.0, 4.0), &none);
        assert_eq!(d.curves.iter().filter(|c| matches!(c, Curve::Arc { .. })).count(), 2);
        let l = profile(&d, &none).unwrap();
        let a = area_of(&l[0].iter().map(|&(x, y)| [x as f64, y as f64]).collect::<Vec<_>>());
        let want = 40.0 + std::f64::consts::PI * 4.0;
        assert!((a - want).abs() < 0.1, "{a} vs {want}");
        // A circle inside a square: two outlines, the hole second.
        let mut d = Drawing::default();
        rectangle(&mut d, [-5.0, -5.0], [5.0, 5.0]);
        let c = add_point(&mut d, [0.0, 0.0]);
        d.curves.push(Curve::Circle { center: c, radius: "2".into() });
        let l = profile(&d, &none).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(circle_through([1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]), Some([0.0, 0.0]));
    }
}
