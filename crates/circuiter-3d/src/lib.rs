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
pub mod math;
pub mod packages;

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

/// Ear-clipping triangulation of a simple counter-clockwise polygon.
fn triangulate(poly: &[(f32, f32)]) -> Vec<[u32; 3]> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    let area2 = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for k in 0..n {
            let (ia, ib, ic) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if area2(a, b, c) <= 1e-9 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                j != ia && j != ib && j != ic && {
                    let p = poly[j];
                    area2(a, b, p) >= 0.0 && area2(b, c, p) >= 0.0 && area2(c, a, p) >= 0.0
                }
            });
            if !inside {
                out.push([ia as u32, ib as u32, ic as u32]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    }
    out
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
