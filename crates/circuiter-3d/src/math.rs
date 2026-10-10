//! Small vector and matrix math: points in millimetres, 4×4 matrices stored by column
//! (as GPUs read them).

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

pub fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l > 1e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}

/// A 4×4 matrix, `m[column][row]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    pub fn translate(t: V3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[3] = [t[0], t[1], t[2], 1.0];
        m
    }

    pub fn scale(s: V3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[0][0] = s[0];
        m.0[1][1] = s[1];
        m.0[2][2] = s[2];
        m
    }

    /// Rotation about the X axis by `a` radians (right-handed).
    pub fn rotate_x(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, c, s, 0.0], [0.0, -s, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub fn rotate_y(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4([[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub fn rotate_z(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4([[c, s, 0.0, 0.0], [-s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    /// `self × o`: applies `o` first, then `self`.
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [[0.0; 4]; 4];
        for (c, col) in r.iter_mut().enumerate() {
            for (row, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[k][row] * o.0[c][k]).sum();
            }
        }
        Mat4(r)
    }

    pub fn point(&self, p: V3) -> V3 {
        let m = &self.0;
        let w = m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3];
        let w = if w.abs() > 1e-12 { w } else { 1.0 };
        [
            (m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0]) / w,
            (m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1]) / w,
            (m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2]) / w,
        ]
    }

    /// A direction: rotated and scaled, not moved.
    pub fn vector(&self, v: V3) -> V3 {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
            m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
            m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
        ]
    }

    /// A right-handed perspective projection to wgpu's clip space (depth 0 to 1).
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fov_y / 2.0).tan();
        let r = far / (near - far);
        Mat4([
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, r, -1.0],
            [0.0, 0.0, r * near, 0.0],
        ])
    }

    /// A camera at `eye` looking at `target`, with `up` towards the top of the view.
    pub fn look_at(eye: V3, target: V3, up: V3) -> Mat4 {
        let f = normalize(sub(target, eye));
        let s = normalize(cross(f, up));
        let u = cross(s, f);
        Mat4([
            [s[0], u[0], -f[0], 0.0],
            [s[1], u[1], -f[1], 0.0],
            [s[2], u[2], -f[2], 0.0],
            [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
    }

    #[test]
    fn rotations_are_right_handed_and_compose() {
        let q = std::f32::consts::FRAC_PI_2;
        assert!(close(Mat4::rotate_z(q).point([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        assert!(close(Mat4::rotate_x(q).point([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(close(Mat4::rotate_y(q).point([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        let m = Mat4::translate([1.0, 2.0, 3.0]).mul(&Mat4::rotate_z(q));
        assert!(close(m.point([1.0, 0.0, 0.0]), [1.0, 3.0, 3.0]));
        assert!(close(m.vector([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn the_camera_looks_down_its_axis() {
        let view = Mat4::look_at([0.0, 0.0, 10.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(view.point([0.0, 0.0, 0.0]), [0.0, 0.0, -10.0]));
        let proj = Mat4::perspective(1.0, 1.0, 1.0, 100.0);
        let near = proj.mul(&view).point([0.0, 0.0, 9.0]);
        let far = proj.mul(&view).point([0.0, 0.0, -90.0]);
        assert!(near[2].abs() < 1e-4 && (far[2] - 1.0).abs() < 1e-4, "{near:?} {far:?}");
    }
}
