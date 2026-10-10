//! Collecting a device's stamps, and (in tests) a dense LU solver the sparse one is
//! checked against.

use crate::device::Stamp;

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct System {
    pub n: usize,
    pub a: Vec<f64>,
    pub b: Vec<f64>,
}

#[cfg(test)]
impl System {
    pub fn new(n: usize) -> Self {
        System { n, a: vec![0.0; n * n], b: vec![0.0; n] }
    }

    /// Solves `A x = b`, returning `x`. On a singular matrix, returns the row (unknown)
    /// that has no usable pivot.
    pub fn solve(&mut self) -> Result<Vec<f64>, usize> {
        let n = self.n;
        let a = &mut self.a;
        let b = &mut self.b;
        let mut perm: Vec<usize> = (0..n).collect();
        for k in 0..n {
            let (p, max) = (k..n)
                .map(|r| (r, a[r * n + k].abs()))
                .fold((k, -1.0), |best, cur| if cur.1 > best.1 { cur } else { best });
            if max <= 1e-300 || !max.is_finite() {
                return Err(perm[k]);
            }
            if p != k {
                for c in 0..n {
                    a.swap(k * n + c, p * n + c);
                }
                b.swap(k, p);
                perm.swap(k, p);
            }
            let pivot = a[k * n + k];
            for r in k + 1..n {
                let f = a[r * n + k] / pivot;
                if f != 0.0 {
                    a[r * n + k] = 0.0;
                    for c in k + 1..n {
                        a[r * n + c] -= f * a[k * n + c];
                    }
                    b[r] -= f * b[k];
                }
            }
        }
        let mut x = vec![0.0; n];
        for k in (0..n).rev() {
            let mut s = b[k];
            for c in k + 1..n {
                s -= a[k * n + c] * x[c];
            }
            x[k] = s / a[k * n + k];
        }
        Ok(x)
    }
}

#[cfg(test)]
impl Stamp for System {
    fn a(&mut self, row: usize, col: usize, v: f64) {
        self.a[row * self.n + col] += v;
    }

    fn b(&mut self, row: usize, v: f64) {
        self.b[row] += v;
    }
}

/// Collects one device's stamps, to recover the currents into its terminals.
#[derive(Debug, Default)]
pub struct Collect {
    pub a: Vec<(usize, usize, f64)>,
    pub b: Vec<(usize, f64)>,
}

impl Stamp for Collect {
    fn a(&mut self, row: usize, col: usize, v: f64) {
        self.a.push((row, col, v));
    }

    fn b(&mut self, row: usize, v: f64) {
        self.b.push((row, v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_and_reports_singular_rows() {
        let mut s = System::new(2);
        s.a = vec![0.0, 2.0, 1.0, 1.0];
        s.b = vec![4.0, 3.0];
        assert_eq!(s.solve().unwrap(), [1.0, 2.0]);
        let mut s = System::new(2);
        s.a = vec![1.0, 0.0, 0.0, 0.0];
        assert_eq!(s.solve(), Err(1));
    }
}
