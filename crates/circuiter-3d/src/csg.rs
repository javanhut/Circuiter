//! Solid booleans: union, difference and intersection of closed meshes, by binary space
//! partitioning (the method of csg.js). Each solid becomes a tree of splitting planes;
//! each solid's polygons are clipped against the other's tree, and what survives makes
//! the result. Works in double precision; faces keep their colour, and normals are
//! interpolated where faces are split.

use crate::{Mesh, Rgba, Vertex};

type P = [f64; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// How close to a plane counts as on it, in millimetres.
const EPSILON: f64 = 1e-5;

#[derive(Debug, Clone, Copy)]
struct V {
    pos: P,
    normal: P,
}

impl V {
    fn flip(&mut self) {
        self.normal = [-self.normal[0], -self.normal[1], -self.normal[2]];
    }
    fn interpolate(&self, o: &V, t: f64) -> V {
        V { pos: lerp(self.pos, o.pos, t), normal: lerp(self.normal, o.normal, t) }
    }
}

#[derive(Debug, Clone, Copy)]
struct Plane {
    normal: P,
    w: f64,
}

impl Plane {
    fn from_points(a: P, b: P, c: P) -> Option<Plane> {
        let n = cross(sub(b, a), sub(c, a));
        let l = dot(n, n).sqrt();
        if l < 1e-12 {
            return None;
        }
        let n = [n[0] / l, n[1] / l, n[2] / l];
        Some(Plane { normal: n, w: dot(n, a) })
    }

    fn flip(&mut self) {
        self.normal = [-self.normal[0], -self.normal[1], -self.normal[2]];
        self.w = -self.w;
    }

    /// Sorts `poly` into the lists by which side of this plane it lies on, splitting it
    /// if it spans the plane.
    fn split(
        &self,
        poly: Polygon,
        coplanar_front: &mut Vec<Polygon>,
        coplanar_back: &mut Vec<Polygon>,
        front: &mut Vec<Polygon>,
        back: &mut Vec<Polygon>,
    ) {
        const COPLANAR: u8 = 0;
        const FRONT: u8 = 1;
        const BACK: u8 = 2;
        const SPANNING: u8 = 3;
        let mut kind = 0u8;
        let types: Vec<u8> = poly
            .vertices
            .iter()
            .map(|v| {
                let t = dot(self.normal, v.pos) - self.w;
                let k = if t < -EPSILON {
                    BACK
                } else if t > EPSILON {
                    FRONT
                } else {
                    COPLANAR
                };
                kind |= k;
                k
            })
            .collect();
        match kind {
            COPLANAR => {
                if dot(self.normal, poly.plane.normal) > 0.0 {
                    coplanar_front.push(poly)
                } else {
                    coplanar_back.push(poly)
                }
            }
            FRONT => front.push(poly),
            BACK => back.push(poly),
            _ => {
                let (mut f, mut b) = (Vec::new(), Vec::new());
                let n = poly.vertices.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (ti, tj) = (types[i], types[j]);
                    let (vi, vj) = (poly.vertices[i], poly.vertices[j]);
                    if ti != BACK {
                        f.push(vi);
                    }
                    if ti != FRONT {
                        b.push(vi);
                    }
                    if (ti | tj) == SPANNING {
                        let t = (self.w - dot(self.normal, vi.pos))
                            / dot(self.normal, sub(vj.pos, vi.pos));
                        let v = vi.interpolate(&vj, t);
                        f.push(v);
                        b.push(v);
                    }
                }
                if f.len() >= 3 {
                    front.push(Polygon { vertices: f, ..poly.clone() });
                }
                if b.len() >= 3 {
                    back.push(Polygon { vertices: b, ..poly });
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Polygon {
    vertices: Vec<V>,
    plane: Plane,
    color: Rgba,
}

impl Polygon {
    fn flip(&mut self) {
        self.vertices.reverse();
        self.vertices.iter_mut().for_each(V::flip);
        self.plane.flip();
    }
}

/// A node of the partitioning tree.
#[derive(Debug, Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polygons: Vec<Polygon>,
}

impl Node {
    fn new(polygons: Vec<Polygon>) -> Node {
        let mut n = Node::default();
        n.build(polygons);
        n
    }

    /// Turns the solid inside out.
    fn invert(&mut self) {
        let mut stack: Vec<&mut Node> = vec![self];
        while let Some(n) = stack.pop() {
            n.polygons.iter_mut().for_each(Polygon::flip);
            if let Some(p) = &mut n.plane {
                p.flip();
            }
            std::mem::swap(&mut n.front, &mut n.back);
            if let Some(f) = n.front.as_deref_mut() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref_mut() {
                stack.push(b);
            }
        }
    }

    /// The parts of `polygons` outside this solid.
    fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = self.plane else { return polygons };
        let (mut front, mut back) = (Vec::new(), Vec::new());
        for p in polygons {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            plane.split(p, &mut cf, &mut cb, &mut front, &mut back);
            front.extend(cf);
            back.extend(cb);
        }
        let front = match &self.front {
            Some(f) => f.clip_polygons(front),
            None => front,
        };
        let back = match &self.back {
            Some(b) => b.clip_polygons(back),
            None => Vec::new(),
        };
        front.into_iter().chain(back).collect()
    }

    /// Removes from this tree every polygon inside `other`.
    fn clip_to(&mut self, other: &Node) {
        let mut stack: Vec<&mut Node> = vec![self];
        while let Some(n) = stack.pop() {
            n.polygons = other.clip_polygons(std::mem::take(&mut n.polygons));
            if let Some(f) = n.front.as_deref_mut() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref_mut() {
                stack.push(b);
            }
        }
    }

    fn all_polygons(&self) -> Vec<Polygon> {
        let mut out = Vec::new();
        let mut stack: Vec<&Node> = vec![self];
        while let Some(n) = stack.pop() {
            out.extend(n.polygons.iter().cloned());
            if let Some(f) = n.front.as_deref() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref() {
                stack.push(b);
            }
        }
        out
    }

    fn build(&mut self, polygons: Vec<Polygon>) {
        if polygons.is_empty() {
            return;
        }
        let plane = *self.plane.get_or_insert(polygons[0].plane);
        let (mut front, mut back) = (Vec::new(), Vec::new());
        let (mut cf, mut cb) = (Vec::new(), Vec::new());
        for p in polygons {
            plane.split(p, &mut cf, &mut cb, &mut front, &mut back);
        }
        self.polygons.extend(cf);
        self.polygons.extend(cb);
        if !front.is_empty() {
            self.front.get_or_insert_with(Box::default).build(front);
        }
        if !back.is_empty() {
            self.back.get_or_insert_with(Box::default).build(back);
        }
    }
}

fn polygons(m: &Mesh) -> Vec<Polygon> {
    let to_p = |v: [f32; 3]| [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])];
    m.indices
        .chunks(3)
        .filter_map(|t| {
            let vs: Vec<&Vertex> = t.iter().map(|&i| &m.vertices[i as usize]).collect();
            let plane = Plane::from_points(to_p(vs[0].pos), to_p(vs[1].pos), to_p(vs[2].pos))?;
            Some(Polygon {
                vertices: vs
                    .iter()
                    .map(|v| V { pos: to_p(v.pos), normal: to_p(v.normal) })
                    .collect(),
                plane,
                color: vs[0].color,
            })
        })
        .collect()
}

fn mesh(polys: Vec<Polygon>) -> Mesh {
    let mut m = Mesh::default();
    let f = |p: P| [p[0] as f32, p[1] as f32, p[2] as f32];
    for p in polys {
        let base = m.vertices.len() as u32;
        for v in &p.vertices {
            let n = v.normal;
            let l = dot(n, n).sqrt();
            let n = if l > 1e-12 { [n[0] / l, n[1] / l, n[2] / l] } else { p.plane.normal };
            m.vertices.push(Vertex { pos: f(v.pos), normal: f(n), color: p.color });
        }
        // Splitting keeps polygons convex, so a fan triangulates them.
        for k in 1..p.vertices.len() as u32 - 1 {
            m.indices.extend([base, base + k, base + k + 1]);
        }
    }
    m
}

/// Runs a boolean on a thread with a large stack: deep trees recurse deeply.
fn run(a: &Mesh, b: &Mesh, op: fn(Node, Node) -> Vec<Polygon>) -> Mesh {
    let (pa, pb) = (polygons(a), polygons(b));
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, move || mesh(op(Node::new(pa), Node::new(pb))))
            .expect("thread")
            .join()
            .expect("boolean")
    })
}

/// Everything in either solid.
pub fn union(a: &Mesh, b: &Mesh) -> Mesh {
    if a.vertices.is_empty() {
        return b.clone();
    }
    if b.vertices.is_empty() {
        return a.clone();
    }
    // Piece by piece, each against only what's near it: a BSP tree's planes reach across
    // all space, so a row of pins, joined whole, would cut every other pin to pieces.
    pieces(b).iter().fold(a.clone(), |out, piece| {
        near(&out, piece, |near| Some(union_whole(&near, piece)), true)
    })
}

/// [`union`] of whole meshes.
fn union_whole(a: &Mesh, b: &Mesh) -> Mesh {
    if a.vertices.is_empty() {
        return b.clone();
    }
    run(a, b, |mut a, mut b| {
        a.clip_to(&b);
        b.clip_to(&a);
        b.invert();
        b.clip_to(&a);
        b.invert();
        a.build(b.all_polygons());
        a.all_polygons()
    })
}

/// What is in `a` and not in `b`. The faces `b` cuts take the colour `cut_color`, or
/// `b`'s own if `None`.
pub fn difference(a: &Mesh, b: &Mesh, cut_color: Option<Rgba>) -> Mesh {
    if a.vertices.is_empty() || b.vertices.is_empty() {
        return a.clone();
    }
    let mut b = b.clone();
    if let Some(c) = cut_color {
        b.vertices.iter_mut().for_each(|v| v.color = c);
    }
    // As union: each piece cut only from what's near it. A piece touching none of
    // `a`'s faces cuts nothing (a hollow sealed inside isn't made).
    pieces(&b).iter().fold(a.clone(), |out, piece| {
        near(&out, piece, |near| Some(difference_whole(&near, piece)), false)
    })
}

/// [`difference`] of whole meshes.
fn difference_whole(a: &Mesh, b: &Mesh) -> Mesh {
    run(a, b, |mut a, mut b| {
        a.invert();
        a.clip_to(&b);
        b.clip_to(&a);
        b.invert();
        b.clip_to(&a);
        b.invert();
        a.build(b.all_polygons());
        a.invert();
        a.all_polygons()
    })
}

/// `a`'s triangles whose bounds reach `piece`'s worked on by `op`, the rest kept as
/// they are. With none near, `piece` is added alongside when `add`, else left out.
fn near(a: &Mesh, piece: &Mesh, op: impl Fn(Mesh) -> Option<Mesh>, add: bool) -> Mesh {
    let Some((lo, hi)) = piece.bounds() else { return a.clone() };
    let reach = 1e-3;
    let (mut close, mut far) = (Mesh::default(), Mesh::default());
    for t in a.indices.chunks_exact(3) {
        let v = [0, 1, 2].map(|k| a.vertices[t[k] as usize]);
        let touches = (0..3).all(|ax| {
            let (tlo, thi) = v
                .iter()
                .fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x.pos[ax]), h.max(x.pos[ax])));
            thi >= lo[ax] - reach && tlo <= hi[ax] + reach
        });
        let into = if touches { &mut close } else { &mut far };
        let base = into.vertices.len() as u32;
        into.vertices.extend(v);
        into.indices.extend([base, base + 1, base + 2]);
    }
    if close.vertices.is_empty() {
        if add {
            far.append(piece.clone());
        }
        return far;
    }
    if let Some(m) = op(close) {
        far.append(m);
    }
    far
}

/// The separate pieces of a mesh: triangles joined through corners at one place.
fn pieces(m: &Mesh) -> Vec<Mesh> {
    let key = |p: [f32; 3]| p.map(|x| (x * 1e4).round() as i64);
    let tris = m.indices.len() / 3;
    let mut parent: Vec<usize> = (0..tris).collect();
    fn find(parent: &mut [usize], mut k: usize) -> usize {
        while parent[k] != k {
            parent[k] = parent[parent[k]];
            k = parent[k];
        }
        k
    }
    let mut first: std::collections::HashMap<[i64; 3], usize> = Default::default();
    for (t, tri) in m.indices.chunks_exact(3).enumerate() {
        for &i in tri {
            let other = *first.entry(key(m.vertices[i as usize].pos)).or_insert(t);
            let (a, b) = (find(&mut parent, t), find(&mut parent, other));
            parent[a] = b;
        }
    }
    let mut out: std::collections::BTreeMap<usize, Mesh> = Default::default();
    for (t, tri) in m.indices.chunks_exact(3).enumerate() {
        let piece = out.entry(find(&mut parent, t)).or_default();
        let base = piece.vertices.len() as u32;
        piece.vertices.extend(tri.iter().map(|&i| m.vertices[i as usize]));
        piece.indices.extend([base, base + 1, base + 2]);
    }
    out.into_values().collect()
}

/// What is in both.
pub fn intersection(a: &Mesh, b: &Mesh) -> Mesh {
    if a.vertices.is_empty() || b.vertices.is_empty() {
        return Mesh::default();
    }
    run(a, b, |mut a, mut b| {
        a.invert();
        b.clip_to(&a);
        b.invert();
        a.clip_to(&b);
        b.clip_to(&a);
        a.build(b.all_polygons());
        a.invert();
        a.all_polygons()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{color, cuboid, cylinder};

    fn volume(m: &Mesh) -> f64 {
        m.indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| {
                    let p = m.vertices[i as usize].pos;
                    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
                });
                dot(a, cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn booleans_have_the_right_volumes() {
        let a = cuboid([0.0; 3], [10.0; 3], color("grey"));
        let b = cuboid([5.0; 3], [15.0; 3], color("red"));
        assert!((volume(&union(&a, &b)) - (2000.0 - 125.0)).abs() < 1e-3);
        assert!((volume(&difference(&a, &b, None)) - (1000.0 - 125.0)).abs() < 1e-3);
        assert!((volume(&intersection(&a, &b)) - 125.0).abs() < 1e-3);
        // A hole through a plate: π r² h less.
        let plate = cuboid([-10.0, -10.0, 0.0], [10.0, 10.0, 2.0], color("grey"));
        let hole = cylinder(3.0, -1.0, 3.0, 64, color("grey"));
        let v = volume(&difference(&plate, &hole, None));
        let want = 800.0 - std::f64::consts::PI * 9.0 * 2.0;
        assert!((v - want).abs() < 0.2, "{v} vs {want}");
    }

    #[test]
    fn cut_faces_take_the_given_colour() {
        let a = cuboid([0.0; 3], [10.0; 3], color("grey"));
        let b = cuboid([5.0; 3], [15.0; 3], color("red"));
        let d = difference(&a, &b, Some(color("grey")));
        assert!(d.vertices.iter().all(|v| v.color == color("grey")));
    }
}
