//! Whether points are inside a closed body: a ray from the point along +X crosses its
//! surface an odd number of times. Triangles are sorted into a grid by where they lie
//! across Y and Z, so each ray meets only those it could cross. Used to check that
//! parts fit: a part's corners inside a case's walls mean the two collide.

use crate::Mesh;
use crate::math::V3;

/// A closed mesh, ready for inside tests.
#[derive(Debug, Clone)]
pub struct Solid {
    tris: Vec<[V3; 3]>,
    lo: [f32; 2],
    cell: f32,
    cols: usize,
    rows: usize,
    buckets: Vec<Vec<u32>>,
    min: V3,
    max: V3,
}

impl Solid {
    pub fn new(mesh: &Mesh) -> Solid {
        let tris: Vec<[V3; 3]> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize].pos))
            .collect();
        let (min, max) = mesh.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
        // About as many cells as triangles, square in Y and Z.
        let (h, w) = ((max[1] - min[1]).max(1e-3), (max[2] - min[2]).max(1e-3));
        let cell = ((h * w) / tris.len().max(1) as f32).sqrt().max(0.05) * 2.0;
        let cols = ((h / cell).ceil() as usize).clamp(1, 512);
        let rows = ((w / cell).ceil() as usize).clamp(1, 512);
        let mut buckets = vec![Vec::new(); cols * rows];
        let at = |y: f32, z: f32| {
            let c = (((y - min[1]) / cell).floor() as isize).clamp(0, cols as isize - 1) as usize;
            let r = (((z - min[2]) / cell).floor() as isize).clamp(0, rows as isize - 1) as usize;
            (c, r)
        };
        for (k, t) in tris.iter().enumerate() {
            let (y0, y1) = (t[0][1].min(t[1][1]).min(t[2][1]), t[0][1].max(t[1][1]).max(t[2][1]));
            let (z0, z1) = (t[0][2].min(t[1][2]).min(t[2][2]), t[0][2].max(t[1][2]).max(t[2][2]));
            let (c0, r0) = at(y0, z0);
            let (c1, r1) = at(y1, z1);
            for c in c0..=c1 {
                for r in r0..=r1 {
                    buckets[r * cols + c].push(k as u32);
                }
            }
        }
        Solid { tris, lo: [min[1], min[2]], cell, cols, rows, buckets, min, max }
    }

    /// Whether `p` is inside.
    pub fn contains(&self, p: V3) -> bool {
        self.crossings(p).0 % 2 == 1
    }

    /// Whether `p` is inside any of the closed pieces the mesh is made of, where they
    /// overlap too (a body built of boxes and cylinders run into each other, which
    /// [`Solid::contains`] would count as outside where two meet): by winding number,
    /// each crossing counted by which way the surface faces.
    pub fn within(&self, p: V3) -> bool {
        self.crossings(p).1 != 0
    }

    /// Crossings of the ray from `p` along +X: how many, and their sum by facing.
    fn crossings(&self, p: V3) -> (u32, i32) {
        if (0..3).any(|k| p[k] < self.min[k] || p[k] > self.max[k]) {
            return (0, 0);
        }
        // Nudged off the grid of round numbers bodies are drawn on, so the ray doesn't
        // run exactly along an edge.
        let (y, z) = (p[1] + 1.37e-4, p[2] + 2.91e-4);
        let c = (((y - self.lo[0]) / self.cell).floor() as isize).clamp(0, self.cols as isize - 1);
        let r = (((z - self.lo[1]) / self.cell).floor() as isize).clamp(0, self.rows as isize - 1);
        let (mut crossings, mut winding) = (0, 0);
        for &k in &self.buckets[r as usize * self.cols + c as usize] {
            let [a, b, d] = self.tris[k as usize];
            // Where (y, z) falls in the triangle seen along X.
            let det = (b[1] - a[1]) * (d[2] - a[2]) - (d[1] - a[1]) * (b[2] - a[2]);
            if det.abs() < 1e-12 {
                continue;
            }
            let u = ((y - a[1]) * (d[2] - a[2]) - (d[1] - a[1]) * (z - a[2])) / det;
            let v = ((b[1] - a[1]) * (z - a[2]) - (y - a[1]) * (b[2] - a[2])) / det;
            if u < 0.0 || v < 0.0 || u + v > 1.0 {
                continue;
            }
            let x = a[0] + u * (b[0] - a[0]) + v * (d[0] - a[0]);
            if x > p[0] {
                crossings += 1;
                winding += if det > 0.0 { 1 } else { -1 };
            }
        }
        (crossings, winding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{color, cuboid, cylinder};

    #[test]
    fn points_inside_and_outside() {
        let block = Solid::new(&cuboid([0.0, 0.0, 0.0], [10.0, 4.0, 2.0], color("grey")));
        assert!(block.contains([5.0, 2.0, 1.0]));
        assert!(block.contains([0.1, 0.1, 0.1]));
        assert!(!block.contains([10.5, 2.0, 1.0]));
        assert!(!block.contains([5.0, 2.0, 2.5]));
        let can = Solid::new(&cylinder(3.0, 0.0, 5.0, 32, color("grey")));
        assert!(can.contains([0.0, 0.0, 2.5]) && can.contains([2.8, 0.0, 4.9]));
        assert!(!can.contains([2.9, 2.9, 2.5]), "outside the round");
        // A hollow box: walls are solid, the cavity isn't.
        let shell = crate::cad::evaluate(
            &[
                circuiter_model::model3d::Feature::new(
                    "Shell",
                    circuiter_model::model3d::Shape::Box {
                        size: ["20".into(), "20".into(), "10".into()],
                        radius: String::new(),
                    },
                ),
                circuiter_model::model3d::Feature {
                    operation: circuiter_model::model3d::Operation::Cut,
                    at: ["0".into(), "0".into(), "2".into()],
                    ..circuiter_model::model3d::Feature::new(
                        "Cavity",
                        circuiter_model::model3d::Shape::Box {
                            size: ["16".into(), "16".into(), "10".into()],
                            radius: String::new(),
                        },
                    )
                },
            ],
            &|_| None,
        );
        let shell = Solid::new(&shell.mesh);
        assert!(!shell.contains([0.0, 0.0, 5.0]), "the cavity is empty");
        assert!(shell.contains([9.0, 0.0, 5.0]), "in the wall");
        assert!(shell.contains([0.0, 0.0, 1.0]), "in the floor");
        // Two boxes run into each other: where they overlap is inside both, which
        // parity alone gets wrong.
        let mut two = cuboid([0.0, 0.0, 0.0], [10.0, 4.0, 2.0], color("grey"));
        two.append(cuboid([2.0, 0.0, 0.0], [4.0, 4.0, 2.0], color("grey")));
        let two = Solid::new(&two);
        assert!(!two.contains([3.0, 2.0, 1.0]) && two.within([3.0, 2.0, 1.0]));
        assert!(two.within([5.5, 2.0, 1.0]) && !two.within([10.5, 2.0, 1.0]));
    }
}
