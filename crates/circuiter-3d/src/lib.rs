//! Circuiter's 3D geometry: triangle meshes with normals and colours, built from simple
//! solids (boxes, cylinders, lathed profiles, extruded outlines, bent wires), the
//! physical bodies of parts made from their parameters ([`packages`]), whole sheets
//! of parts laid out in 3D ([`assembly`]), and STL export.
//!
//! Units are millimetres. Z is up; a board's top surface is z = 0, so bodies sit above
//! it and through-hole leads go below.

pub mod assembly;
pub mod cad;
pub mod csg;
pub mod import2d;
pub mod math;
pub mod packages;
pub mod solid;

use math::{Mat4, V3, cross, normalize, sub};

/// An RGBA colour, 8 bits per channel.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: V3,
    pub normal: V3,
    pub color: Rgba,
}

/// Triangles: every three indices make one, counter-clockwise seen from outside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn append(&mut self, other: Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(other.vertices);
        self.indices.extend(other.indices.into_iter().map(|i| i + base));
    }

    pub fn with(mut self, other: Mesh) -> Mesh {
        self.append(other);
        self
    }

    /// Moves, turns or scales every vertex. A mirroring transform flips the winding.
    pub fn transformed(mut self, m: &Mat4) -> Mesh {
        let normal = normal_matrix(m);
        for v in &mut self.vertices {
            v.pos = m.point(v.pos);
            v.normal = normalize(normal.vector(v.normal));
        }
        let det = determinant3(m);
        if det < 0.0 {
            for t in self.indices.chunks_mut(3) {
                t.swap(1, 2);
            }
        }
        self
    }

    pub fn translated(self, t: V3) -> Mesh {
        self.transformed(&Mat4::translate(t))
    }

    /// The smallest box holding every vertex, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(V3, V3)> {
        let first = self.vertices.first()?.pos;
        Some(self.vertices.iter().fold((first, first), |(lo, hi), v| {
            (
                [lo[0].min(v.pos[0]), lo[1].min(v.pos[1]), lo[2].min(v.pos[2])],
                [hi[0].max(v.pos[0]), hi[1].max(v.pos[1]), hi[2].max(v.pos[2])],
            )
        }))
    }

    fn push(&mut self, pos: V3, normal: V3, color: Rgba) -> u32 {
        self.vertices.push(Vertex { pos, normal, color });
        self.vertices.len() as u32 - 1
    }

    /// What lies on one side of a plane square to `axis` at `at`: the side above it
    /// when `keep_above`. Triangles across the plane are cut along it (the cut isn't
    /// capped: a section shows the walls' thickness as an open edge).
    pub fn clipped(&self, axis: usize, at: f32, keep_above: bool) -> Mesh {
        let side = |v: &Vertex| if keep_above { v.pos[axis] - at } else { at - v.pos[axis] };
        let mut out = Mesh::default();
        for t in self.indices.chunks_exact(3) {
            let vs = [t[0], t[1], t[2]].map(|i| self.vertices[i as usize]);
            let d = vs.map(|v| side(&v));
            if d.iter().all(|&x| x >= 0.0) {
                let i = vs.map(|v| out.push(v.pos, v.normal, v.color));
                out.indices.extend(i);
                continue;
            }
            if d.iter().all(|&x| x <= 0.0) {
                continue;
            }
            // Sutherland–Hodgman against the plane, then a fan.
            let mut poly: Vec<Vertex> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (vs[k], vs[(k + 1) % 3]);
                let (da, db) = (d[k], d[(k + 1) % 3]);
                if da >= 0.0 {
                    poly.push(a);
                }
                if (da >= 0.0) != (db >= 0.0) {
                    let s = da / (da - db);
                    let lerp = |p: V3, q: V3| [0, 1, 2].map(|j| p[j] + (q[j] - p[j]) * s);
                    poly.push(Vertex { pos: lerp(a.pos, b.pos), normal: a.normal, color: a.color });
                }
            }
            let base: Vec<u32> = poly.iter().map(|v| out.push(v.pos, v.normal, v.color)).collect();
            for k in 1..base.len().saturating_sub(1) {
                out.indices.extend([base[0], base[k], base[k + 1]]);
            }
        }
        out
    }

    /// A flat quad `a b c d`, counter-clockwise seen from outside.
    fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, color: Rgba) {
        let n = normalize(cross(sub(b, a), sub(d, a)));
        let i = [a, b, c, d].map(|p| self.push(p, n, color));
        self.indices.extend([i[0], i[1], i[2], i[0], i[2], i[3]]);
    }

    /// Binary STL: what slicers, viewers and CAD programs read.
    pub fn to_stl(&self, name: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(84 + 50 * self.triangles());
        let mut header = [0u8; 80];
        let text = format!("Circuiter {name}");
        let n = text.len().min(80);
        header[..n].copy_from_slice(&text.as_bytes()[..n]);
        out.extend_from_slice(&header);
        out.extend_from_slice(&(self.triangles() as u32).to_le_bytes());
        for t in self.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.vertices[i as usize].pos);
            let n = normalize(cross(sub(b, a), sub(c, a)));
            for v in [n, a, b, c] {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }
}

/// The inverse transpose of the upper 3×3, for normals.
fn normal_matrix(m: &Mat4) -> Mat4 {
    let a = |c: usize, r: usize| m.0[c][r];
    let det = determinant3(m);
    let inv = if det.abs() > 1e-12 { 1.0 / det } else { 1.0 };
    // Cofactors of the 3×3, which is the inverse transpose times the determinant.
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| a(c0, r0) * a(c1, r1) - a(c1, r0) * a(c0, r1);
    let mut out = Mat4::IDENTITY;
    for c in 0..3 {
        for r in 0..3 {
            let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
            let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
            out.0[c][r] = cof(r0, r1, c0, c1) * inv;
        }
    }
    out
}

fn determinant3(m: &Mat4) -> f32 {
    let a = |c: usize, r: usize| m.0[c][r];
    a(0, 0) * (a(1, 1) * a(2, 2) - a(2, 1) * a(1, 2))
        - a(1, 0) * (a(0, 1) * a(2, 2) - a(2, 1) * a(0, 2))
        + a(2, 0) * (a(0, 1) * a(1, 2) - a(1, 1) * a(0, 2))
}

// ---- Solids --------------------------------------------------------------------------

/// A box between two corners.
pub fn cuboid(lo: V3, hi: V3, color: Rgba) -> Mesh {
    let mut m = Mesh::default();
    let [x0, y0, z0] = lo;
    let [x1, y1, z1] = hi;
    m.quad([x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0], color); // bottom
    m.quad([x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1], color); // top
    m.quad([x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1], color); // front
    m.quad([x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1], color); // back
    m.quad([x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1], color); // left
    m.quad([x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1], color); // right
    m
}

/// A profile of `(radius, z)` points turned about the Z axis, from the bottom up. A
/// point with radius 0 closes the solid there.
pub fn lathe(profile: &[(f32, f32)], segments: usize, color: Rgba) -> Mesh {
    lathe_sweep(profile, std::f32::consts::TAU, segments, color)
}

/// [`lathe`] through `sweep` radians only, counter-clockwise from +X. The ends are left
/// open (see [`revolve`] for a closed solid).
pub fn lathe_sweep(profile: &[(f32, f32)], sweep: f32, segments: usize, color: Rgba) -> Mesh {
    let mut m = Mesh::default();
    let n = segments.max(3);
    // Each band's normal in the (radius, z) plane, outward from the profile.
    let bands: Vec<(f32, f32)> = profile
        .windows(2)
        .map(|w| {
            let (dr, dz) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            let l = (dr * dr + dz * dz).sqrt().max(1e-9);
            (dz / l, -dr / l)
        })
        .collect();
    // Where neighbouring bands meet at a gentle angle, they share the averaged normal,
    // so curves shade smoothly; a sharp corner (more than about 35°) stays sharp.
    let joint = |a: (f32, f32), b: (f32, f32)| {
        if a.0 * b.0 + a.1 * b.1 > 0.82 {
            let (x, y) = (a.0 + b.0, a.1 + b.1);
            let l = (x * x + y * y).sqrt().max(1e-9);
            (x / l, y / l)
        } else {
            a
        }
    };
    for (k, w) in profile.windows(2).enumerate() {
        let ((r0, z0), (r1, z1)) = (w[0], w[1]);
        let here = bands[k];
        let n0 = if k > 0 { joint(here, bands[k - 1]) } else { here };
        let n1 = if k + 1 < bands.len() { joint(here, bands[k + 1]) } else { here };
        for j in 0..n {
            let a0 = sweep * j as f32 / n as f32;
            let a1 = sweep * (j + 1) as f32 / n as f32;
            let (s0, c0) = a0.sin_cos();
            let (s1, c1) = a1.sin_cos();
            let p = |r: f32, z: f32, c: f32, s: f32| [r * c, r * s, z];
            let nrm = |(nr, nz): (f32, f32), c: f32, s: f32| normalize([nr * c, nr * s, nz]);
            let i = [
                m.push(p(r0, z0, c0, s0), nrm(n0, c0, s0), color),
                m.push(p(r0, z0, c1, s1), nrm(n0, c1, s1), color),
                m.push(p(r1, z1, c1, s1), nrm(n1, c1, s1), color),
                m.push(p(r1, z1, c0, s0), nrm(n1, c0, s0), color),
            ];
            m.indices.extend([i[0], i[1], i[2], i[0], i[2], i[3]]);
        }
    }
    m
}

/// A closed outline in the (radius, height) plane turned about the Z axis by `sweep`
/// radians into a solid; short of a full turn, its two ends are capped.
pub fn revolve(outline: &[(f32, f32)], sweep: f32, segments: usize, color: Rgba) -> Mesh {
    let mut loop_ = orient_ccw(outline);
    if loop_.len() < 3 {
        return Mesh::default();
    }
    loop_.iter_mut().for_each(|p| p.0 = p.0.max(0.0));
    let first = loop_[0];
    loop_.push(first);
    let full = sweep >= std::f32::consts::TAU - 1e-4;
    let sweep = sweep.min(std::f32::consts::TAU);
    let n = ((segments as f32) * sweep / std::f32::consts::TAU).ceil().max(2.0) as usize;
    let mut m = lathe_sweep(&loop_, sweep, n, color);
    if !full {
        loop_.pop();
        let tris = triangulate(&loop_);
        // The start cap faces −Y (back along the turn), the end cap forward.
        for (angle, outward) in [(0.0f32, -1.0f32), (sweep, 1.0)] {
            let (s, c) = angle.sin_cos();
            let normal = [-s * outward, c * outward, 0.0];
            let base = m.vertices.len() as u32;
            for &(r, z) in &loop_ {
                m.push([r * c, r * s, z], normal, color);
            }
            // A counter-clockwise outline at angle 0 already faces −Y.
            for t in &tris {
                if outward < 0.0 {
                    m.indices.extend([base + t[0], base + t[1], base + t[2]]);
                } else {
                    m.indices.extend([base + t[0], base + t[2], base + t[1]]);
                }
            }
        }
    }
    m
}

/// `outline` running counter-clockwise.
pub fn orient_ccw(outline: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let n = outline.len();
    let area: f32 = (0..n)
        .map(|k| {
            let (a, b) = (outline[k], outline[(k + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let mut v = outline.to_vec();
    if area < 0.0 {
        v.reverse();
    }
    v
}

/// A closed cylinder about the Z axis.
pub fn cylinder(r: f32, z0: f32, z1: f32, segments: usize, color: Rgba) -> Mesh {
    lathe(&[(0.0, z0), (r, z0), (r, z1), (0.0, z1)], segments, color)
}

/// A cylinder of radius `r` from `a` to `b`.
pub fn rod(a: V3, b: V3, r: f32, segments: usize, color: Rgba) -> Mesh {
    let d = sub(b, a);
    let len = math::length(d);
    if len < 1e-6 {
        return Mesh::default();
    }
    cylinder(r, 0.0, len, segments, color).transformed(&align_z(d)).translated(a)
}

/// A rotation taking +Z onto direction `d`.
pub fn align_z(d: V3) -> Mat4 {
    let z = normalize(d);
    let helper = if z[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let x = normalize(cross(helper, z));
    let y = cross(z, x);
    Mat4([
        [x[0], x[1], x[2], 0.0],
        [y[0], y[1], y[2], 0.0],
        [z[0], z[1], z[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

/// A wire of radius `r` along `path`, with round joints so bends look solid.
pub fn wire(path: &[V3], r: f32, color: Rgba) -> Mesh {
    let mut m = Mesh::default();
    for w in path.windows(2) {
        m.append(rod(w[0], w[1], r, 10, color));
    }
    for p in path.iter().skip(1).take(path.len().saturating_sub(2)) {
        m.append(sphere(*p, r, 10, color));
    }
    m
}

pub fn sphere(center: V3, r: f32, segments: usize, color: Rgba) -> Mesh {
    let rings = (segments / 2).max(3);
    let profile: Vec<(f32, f32)> = (0..=rings)
        .map(|k| {
            let a = std::f32::consts::PI * (k as f32 / rings as f32) - std::f32::consts::FRAC_PI_2;
            (r * a.cos(), r * a.sin())
        })
        .collect();
    lathe(&profile, segments, color).translated(center)
}

/// A flat outline (counter-clockwise, in the XY plane) raised from `z0` to `z1`.
pub fn extrude(outline: &[(f32, f32)], z0: f32, z1: f32, color: Rgba) -> Mesh {
    let outline = &orient_ccw(outline)[..];
    let mut m = Mesh::default();
    let n = outline.len();
    if n < 3 {
        return m;
    }
    // Each side's outward normal; where neighbouring sides meet at a gentle angle
    // (a curve made of segments) they share the averaged one, so curves shade smoothly.
    let side: Vec<V3> = (0..n)
        .map(|k| {
            let (a, b) = (outline[k], outline[(k + 1) % n]);
            normalize([b.1 - a.1, a.0 - b.0, 0.0])
        })
        .collect();
    let blend = |s: V3, other: V3| {
        if math::dot(s, other) > 0.82 { normalize(math::add(s, other)) } else { s }
    };
    for k in 0..n {
        let (a, b) = (outline[k], outline[(k + 1) % n]);
        let na = blend(side[k], side[(k + n - 1) % n]);
        let nb = blend(side[k], side[(k + 1) % n]);
        let i = [
            m.push([a.0, a.1, z0], na, color),
            m.push([b.0, b.1, z0], nb, color),
            m.push([b.0, b.1, z1], nb, color),
            m.push([a.0, a.1, z1], na, color),
        ];
        m.indices.extend([i[0], i[1], i[2], i[0], i[2], i[3]]);
    }
    let tris = triangulate(outline);
    for (z, up) in [(z1, true), (z0, false)] {
        let normal = if up { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] };
        let base = m.vertices.len() as u32;
        for p in outline {
            m.push([p.0, p.1, z], normal, color);
        }
        for t in &tris {
            if up {
                m.indices.extend([base + t[0], base + t[1], base + t[2]]);
            } else {
                m.indices.extend([base + t[0], base + t[2], base + t[1]]);
            }
        }
    }
    m
}

/// Ear-clipping triangulation of a simple counter-clockwise polygon. Repeated points
/// don't block ears at their own place.
fn triangulate(poly: &[(f32, f32)]) -> Vec<[u32; 3]> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    let area2 = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let mut k = 0;
    let mut misses = 0;
    while idx.len() > 3 && misses <= idx.len() {
        let n = idx.len();
        k %= n;
        let (ia, ib, ic) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
        let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
        let convex = area2(a, b, c) > 1e-9;
        let (lo, hi) = (
            (a.0.min(b.0).min(c.0), a.1.min(b.1).min(c.1)),
            (a.0.max(b.0).max(c.0), a.1.max(b.1).max(c.1)),
        );
        let blocked = convex
            && idx.iter().any(|&j| {
                let p = poly[j];
                p.0 >= lo.0
                    && p.0 <= hi.0
                    && p.1 >= lo.1
                    && p.1 <= hi.1
                    && p != a
                    && p != b
                    && p != c
                    && area2(a, b, p) >= 0.0
                    && area2(b, c, p) >= 0.0
                    && area2(c, a, p) >= 0.0
            });
        if convex && !blocked {
            out.push([ia as u32, ib as u32, ic as u32]);
            idx.remove(k);
            misses = 0;
        } else if area2(a, b, c).abs() <= 1e-9 && (a == c || a == b || b == c) {
            // A spike or a repeated point: drop it.
            idx.remove(k);
            misses = 0;
        } else {
            k += 1;
            misses += 1;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    }
    out
}

/// Triangles covering an outline less its holes (any orientation; inside is where an
/// odd number of outlines enclose): the plane cut into horizontal slabs at every
/// corner's height, each slab's inside spans made two triangles. Robust whatever the
/// holes, at the price of corners shared along slab edges. Returns the points and the
/// triangles over them, counter-clockwise.
pub fn triangulate_holes(
    outer: &[(f32, f32)],
    holes: &[Vec<(f32, f32)>],
) -> (Vec<(f32, f32)>, Vec<[u32; 3]>) {
    // Edges that aren't flat, lower end first.
    let mut edges: Vec<[f64; 4]> = Vec::new();
    for ring in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
        let n = ring.len();
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            let (a, b) = ((f64::from(a.0), f64::from(a.1)), (f64::from(b.0), f64::from(b.1)));
            if a.1 != b.1 {
                edges.push(if a.1 < b.1 { [a.0, a.1, b.0, b.1] } else { [b.0, b.1, a.0, a.1] });
            }
        }
    }
    edges.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut ys: Vec<f64> = edges.iter().flat_map(|e| [e[1], e[3]]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    let x_at = |e: &[f64; 4], y: f64| e[0] + (e[2] - e[0]) * (y - e[1]) / (e[3] - e[1]);
    let mut points = Vec::new();
    let mut tris = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    for w in ys.windows(2) {
        let (ya, yb) = (w[0], w[1]);
        let ym = (ya + yb) / 2.0;
        while next < edges.len() && edges[next][1] < ym {
            active.push(next);
            next += 1;
        }
        active.retain(|&k| edges[k][3] > ym);
        let mut spans: Vec<(f64, f64, f64)> = active
            .iter()
            .map(|&k| (x_at(&edges[k], ym), x_at(&edges[k], ya), x_at(&edges[k], yb)))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in spans.chunks_exact(2) {
            let (l, r) = (pair[0], pair[1]);
            let base = points.len() as u32;
            points.extend([
                (l.1 as f32, ya as f32),
                (r.1 as f32, ya as f32),
                (r.2 as f32, yb as f32),
                (l.2 as f32, yb as f32),
            ]);
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
        }
    }
    (points, tris)
}

/// A flat outline with holes raised from `z0` to `z1`: walls round the outside and
/// inside each hole, caps top and bottom.
pub fn extrude_holes(
    outer: &[(f32, f32)],
    holes: &[Vec<(f32, f32)>],
    z0: f32,
    z1: f32,
    color: Rgba,
) -> Mesh {
    if holes.is_empty() {
        return extrude(outer, z0, z1, color);
    }
    let mut m = Mesh::default();
    let mut walls = |ring: &[(f32, f32)]| {
        let n = ring.len();
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            let normal = normalize([b.1 - a.1, a.0 - b.0, 0.0]);
            let i = [
                m.push([a.0, a.1, z0], normal, color),
                m.push([b.0, b.1, z0], normal, color),
                m.push([b.0, b.1, z1], normal, color),
                m.push([a.0, a.1, z1], normal, color),
            ];
            m.indices.extend([i[0], i[1], i[2], i[0], i[2], i[3]]);
        }
    };
    walls(&orient_ccw(outer));
    for h in holes.iter().filter(|h| h.len() >= 3) {
        let mut h = orient_ccw(h);
        h.reverse();
        walls(&h);
    }
    let (poly, tris) = triangulate_holes(outer, holes);
    for (z, up) in [(z1, true), (z0, false)] {
        let normal = if up { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] };
        let base = m.vertices.len() as u32;
        for p in &poly {
            m.push([p.0, p.1, z], normal, color);
        }
        for t in &tris {
            if up {
                m.indices.extend([base + t[0], base + t[1], base + t[2]]);
            } else {
                m.indices.extend([base + t[0], base + t[2], base + t[1]]);
            }
        }
    }
    m
}

/// A rectangle with rounded corners, counter-clockwise, centred on the origin.
pub fn rounded_rect(w: f32, h: f32, r: f32, per_corner: usize) -> Vec<(f32, f32)> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut out = Vec::new();
    let corners = [
        (w / 2.0 - r, -h / 2.0 + r, -90.0f32),
        (w / 2.0 - r, h / 2.0 - r, 0.0),
        (-w / 2.0 + r, h / 2.0 - r, 90.0),
        (-w / 2.0 + r, -h / 2.0 + r, 180.0),
    ];
    for (cx, cy, start) in corners {
        for k in 0..=per_corner {
            let a = (start + 90.0 * k as f32 / per_corner as f32).to_radians();
            out.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    out
}

/// Named colours for bodies and markings.
pub fn color(name: &str) -> Rgba {
    match name.trim().to_ascii_lowercase().as_str() {
        "black" => [24, 24, 26, 255],
        "brown" => [120, 66, 30, 255],
        "red" => [205, 32, 30, 255],
        "orange" => [235, 120, 20, 255],
        "yellow" => [240, 205, 30, 255],
        "green" => [30, 150, 60, 255],
        "blue" => [30, 80, 200, 255],
        "violet" | "purple" => [130, 60, 170, 255],
        "grey" | "gray" => [128, 128, 128, 255],
        "white" => [235, 235, 230, 255],
        "gold" => [200, 160, 60, 255],
        "silver" | "tin" => [196, 198, 202, 255],
        "copper" => [184, 115, 51, 255],
        "beige" => [222, 196, 150, 255],
        "glass" => [225, 120, 70, 255],
        "clear" => [215, 225, 230, 255],
        "board" => [22, 96, 52, 255],
        "lightblue" => [120, 170, 220, 255],
        "darkblue" => [20, 40, 110, 255],
        "aluminium" | "aluminum" => [205, 208, 212, 255],
        _ => [150, 150, 150, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_are_cut_in_section() {
        let block = cuboid([0.0, 0.0, 0.0], [10.0, 4.0, 2.0], color("grey"));
        let half = block.clipped(0, 4.0, true);
        let (lo, hi) = half.bounds().unwrap();
        assert!((lo[0] - 4.0).abs() < 1e-5 && (hi[0] - 10.0).abs() < 1e-5);
        let other = block.clipped(0, 4.0, false);
        assert!((other.bounds().unwrap().1[0] - 4.0).abs() < 1e-5);
        assert!(block.clipped(0, 20.0, true).indices.is_empty());
    }

    #[test]
    fn outlines_with_holes_extrude_closed() {
        let square = |c: (f32, f32), r: f32| {
            vec![(c.0 - r, c.1 - r), (c.0 + r, c.1 - r), (c.0 + r, c.1 + r), (c.0 - r, c.1 + r)]
        };
        let circle = |c: (f32, f32), r: f32| -> Vec<(f32, f32)> {
            (0..32)
                .map(|k| {
                    let a = k as f32 * std::f32::consts::TAU / 32.0;
                    (c.0 + r * a.cos(), c.1 + r * a.sin())
                })
                .collect()
        };
        let holes = vec![
            square((-5.0, 0.0), 2.0),
            circle((5.0, 3.0), 2.0),
            circle((5.0, -4.0), 1.5),
            square((0.0, 0.0), 1.0),
        ];
        let m = extrude_holes(&square((0.0, 0.0), 10.0), &holes, 0.0, 1.0, color("board"));
        let hole_area = 16.0 + 4.0 + std::f32::consts::PI * (4.0 + 2.25) * 0.994;
        let want = 400.0 - hole_area;
        assert!((volume(&m) - want).abs() < 0.5, "{} vs {want}", volume(&m));
        // The top alone covers exactly the area between: every triangle of it counted once.
        let top: f32 = m
            .indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize]);
                if a.normal[2] > 0.5 {
                    ((b.pos[0] - a.pos[0]) * (c.pos[1] - a.pos[1])
                        - (b.pos[1] - a.pos[1]) * (c.pos[0] - a.pos[0]))
                        / 2.0
                } else {
                    0.0
                }
            })
            .sum();
        assert!((top - want).abs() < 0.5, "{top} vs {want}");
        // A pour with a hundred clearance holes is covered exactly.
        let many: Vec<_> = (0..100)
            .map(|k| circle((-9.0 + 2.0 * (k % 10) as f32, -9.0 + 2.0 * (k / 10) as f32), 0.6))
            .collect();
        let (pts, tris) = triangulate_holes(&square((0.0, 0.0), 10.0), &many);
        let area: f32 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| pts[i as usize]);
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)) / 2.0
            })
            .sum();
        let want = 400.0 - 100.0 * std::f32::consts::PI * 0.36 * 0.994;
        assert!((area - want).abs() < 0.5, "{area} vs {want}");
    }

    /// A closed mesh's signed volume, positive when its faces point outwards.
    pub fn volume(m: &Mesh) -> f32 {
        m.indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                math::dot(a, cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn solids_are_closed_and_face_outwards() {
        let b = cuboid([0.0; 3], [2.0, 3.0, 4.0], color("red"));
        assert!((volume(&b) - 24.0).abs() < 1e-4, "{}", volume(&b));
        let c = cylinder(1.0, 0.0, 2.0, 64, color("red"));
        assert!((volume(&c) - std::f32::consts::PI * 2.0).abs() < 0.02, "{}", volume(&c));
        let e = extrude(
            &[(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 2.0), (0.0, 1.0)],
            0.0,
            1.0,
            color("red"),
        );
        assert!((volume(&e) - 3.0).abs() < 1e-4, "{}", volume(&e));
        // Turned upside down by a mirror, it still faces out.
        let mirrored = b.clone().transformed(&Mat4::scale([1.0, 1.0, -1.0]));
        assert!(volume(&mirrored) > 0.0);
        let r = rod([0.0, 0.0, 0.0], [3.0, 4.0, 0.0], 0.5, 32, color("red"));
        let (lo, hi) = r.bounds().unwrap();
        assert!((hi[0] - 3.4).abs() < 0.11 && lo[0] > -0.5, "{lo:?} {hi:?}");
    }

    #[test]
    fn stl_has_a_header_a_count_and_fifty_bytes_a_triangle() {
        let m = cuboid([0.0; 3], [1.0; 3], color("red"));
        let stl = m.to_stl("box");
        assert_eq!(stl.len(), 84 + 50 * 12);
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 12);
        assert!(stl.starts_with(b"Circuiter box"));
    }
}
