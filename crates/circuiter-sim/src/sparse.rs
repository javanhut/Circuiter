//! Sparse LU factorisation for circuit matrices, in the manner of KLU.
//!
//! - A minimum-degree ordering of the symmetrised structure keeps fill-in low.
//! - Left-looking Gilbert–Peierls factorisation with threshold partial pivoting, which
//!   prefers the diagonal but copes with the zero diagonals of voltage-source rows.
//! - Refactorisation: the structure and pivot order are kept, and only the numbers are
//!   recomputed, which is what every Newton iteration after the first needs.
//!
//! Generic over the scalar, so AC analysis uses it with complex numbers.

use crate::device::Stamp;
use std::collections::{BinaryHeap, HashSet};
use std::ops::{Add, Div, Mul, Neg, Sub};

pub trait Scalar:
    Copy
    + Default
    + PartialEq
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    fn magnitude(self) -> f64;
}

impl Scalar for f64 {
    fn magnitude(self) -> f64 {
        self.abs()
    }
}

/// The positions of a matrix's nonzeros, column by column (compressed sparse column),
/// rows sorted within each column. Every diagonal entry is present.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub n: usize,
    colptr: Vec<usize>,
    rows: Vec<usize>,
}

impl Pattern {
    pub fn new(n: usize, entries: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut cols: Vec<Vec<usize>> = (0..n).map(|c| vec![c]).collect();
        for (r, c) in entries {
            cols[c].push(r);
        }
        let mut colptr = Vec::with_capacity(n + 1);
        let mut rows = Vec::new();
        colptr.push(0);
        for mut col in cols {
            col.sort_unstable();
            col.dedup();
            rows.extend(col);
            colptr.push(rows.len());
        }
        Pattern { n, colptr, rows }
    }

    pub fn nnz(&self) -> usize {
        self.rows.len()
    }

    /// The storage index of entry (`r`, `c`), if it is in the pattern.
    pub fn find(&self, r: usize, c: usize) -> Option<usize> {
        let (a, b) = (self.colptr[c], self.colptr[c + 1]);
        self.rows[a..b].binary_search(&r).ok().map(|k| a + k)
    }

    fn column(&self, c: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        (self.colptr[c]..self.colptr[c + 1]).map(move |k| (self.rows[k], k))
    }

    pub fn entries(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.n).flat_map(move |c| self.column(c).map(move |(r, _)| (r, c)))
    }
}

/// A linear system `A x = b` assembled into a fixed pattern. Stamps outside the
/// pattern are kept aside ([`SparseSystem::missing`]) so the caller can grow the
/// pattern and assemble again.
#[derive(Debug, Clone)]
pub struct SparseSystem {
    pub pattern: Pattern,
    pub values: Vec<f64>,
    pub b: Vec<f64>,
    pub missing: Vec<(usize, usize)>,
}

impl SparseSystem {
    pub fn new(pattern: Pattern) -> Self {
        let (nnz, n) = (pattern.nnz(), pattern.n);
        SparseSystem { pattern, values: vec![0.0; nnz], b: vec![0.0; n], missing: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.values.fill(0.0);
        self.b.fill(0.0);
        self.missing.clear();
    }
}

impl Stamp for SparseSystem {
    fn a(&mut self, row: usize, col: usize, v: f64) {
        match self.pattern.find(row, col) {
            Some(k) => self.values[k] += v,
            None => self.missing.push((row, col)),
        }
    }

    fn b(&mut self, row: usize, v: f64) {
        self.b[row] += v;
    }
}

/// Minimum-degree ordering of the graph of `A + Aᵀ`: repeatedly eliminates the node
/// with fewest neighbours, joining its neighbours to each other (the fill it causes).
pub fn minimum_degree(p: &Pattern) -> Vec<usize> {
    let n = p.n;
    let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (r, c) in p.entries() {
        if r != c {
            adj[r].insert(c);
            adj[c].insert(r);
        }
    }
    let mut heap: BinaryHeap<std::cmp::Reverse<(usize, usize)>> =
        (0..n).map(|v| std::cmp::Reverse((adj[v].len(), v))).collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while let Some(std::cmp::Reverse((deg, v))) = heap.pop() {
        if done[v] || deg != adj[v].len() {
            continue; // Stale entry.
        }
        done[v] = true;
        order.push(v);
        let neighbours: Vec<usize> = adj[v].drain().collect();
        for &a in &neighbours {
            adj[a].remove(&v);
        }
        for (i, &a) in neighbours.iter().enumerate() {
            for &b in &neighbours[i + 1..] {
                adj[a].insert(b);
                adj[b].insert(a);
            }
        }
        for &a in &neighbours {
            heap.push(std::cmp::Reverse((adj[a].len(), a)));
        }
    }
    order
}

/// A factorisation `P A Q = L U`, with `L` unit lower triangular.
#[derive(Debug, Clone)]
pub struct Lu<T> {
    n: usize,
    /// Column order: step `k` factors column `q[k]` of `A`.
    q: Vec<usize>,
    /// The row chosen as pivot at each step, and its inverse.
    prow: Vec<usize>,
    pinv: Vec<usize>,
    /// Below-diagonal entries of each column of `L`, by original row.
    l: Vec<Vec<(usize, T)>>,
    /// Above-diagonal entries of each column of `U`, by step, in the order they are
    /// computed (a topological order, which refactorisation replays).
    u: Vec<Vec<(usize, T)>>,
    diag: Vec<T>,
}

const UNSET: usize = usize::MAX;

impl<T: Scalar> Lu<T> {
    /// Factorises `values` (in `pattern`) with columns taken in `order`. Within a
    /// column, the pivot is the diagonal if it is at least `threshold` times the largest
    /// candidate, else the largest. Fails with the column that has no usable pivot.
    pub fn factor(
        pattern: &Pattern,
        values: &[T],
        order: &[usize],
        threshold: f64,
    ) -> Result<Self, usize> {
        let n = pattern.n;
        let mut lu = Lu {
            n,
            q: order.to_vec(),
            prow: vec![UNSET; n],
            pinv: vec![UNSET; n],
            l: vec![Vec::new(); n],
            u: vec![Vec::new(); n],
            diag: vec![T::default(); n],
        };
        let mut x = vec![T::default(); n];
        let mut mark = vec![UNSET; n];
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut topo: Vec<usize> = Vec::new();
        let mut candidates: Vec<usize> = Vec::new();
        for k in 0..n {
            let j = lu.q[k];
            // Reach: every row the solve of L x = A(:, j) can touch, pivotal rows in
            // topological order (dependencies first).
            topo.clear();
            candidates.clear();
            for (r, idx) in pattern.column(j) {
                x[r] = values[idx];
                if mark[r] == k {
                    continue;
                }
                mark[r] = k;
                if lu.pinv[r] == UNSET {
                    candidates.push(r);
                    continue;
                }
                stack.push((r, 0));
                while let Some(&mut (node, ref mut next)) = stack.last_mut() {
                    let col = &lu.l[lu.pinv[node]];
                    if let Some(&(child, _)) = col.get(*next) {
                        *next += 1;
                        if mark[child] != k {
                            mark[child] = k;
                            if lu.pinv[child] == UNSET {
                                candidates.push(child);
                            } else {
                                stack.push((child, 0));
                            }
                        }
                    } else {
                        stack.pop();
                        topo.push(node);
                    }
                }
            }
            // Eliminate in dependency order.
            for &r in topo.iter().rev() {
                let kk = lu.pinv[r];
                let xr = x[r];
                lu.u[k].push((kk, xr));
                for &(i, lv) in &lu.l[kk] {
                    x[i] = x[i] - lv * xr;
                }
                x[r] = T::default();
            }
            // Pivot.
            let max = candidates.iter().map(|&r| x[r].magnitude()).fold(0.0, f64::max);
            if max <= 1e-300 || !max.is_finite() {
                return Err(j);
            }
            let diagonal = candidates.iter().copied().find(|&r| r == j);
            let pivot = match diagonal {
                Some(r) if x[r].magnitude() >= threshold * max => r,
                _ => candidates
                    .iter()
                    .copied()
                    .max_by(|&a, &b| x[a].magnitude().total_cmp(&x[b].magnitude()))
                    .expect("max > 0"),
            };
            let d = x[pivot];
            lu.diag[k] = d;
            lu.prow[k] = pivot;
            lu.pinv[pivot] = k;
            for &r in &candidates {
                if r != pivot {
                    lu.l[k].push((r, x[r] / d));
                }
                x[r] = T::default();
            }
        }
        Ok(lu)
    }

    /// Factorises new values with the same structure and pivots. Returns false (and the
    /// caller should [`Lu::factor`] again) if a pivot has become too small.
    #[must_use]
    pub fn refactor(&mut self, pattern: &Pattern, values: &[T]) -> bool {
        let mut x = vec![T::default(); self.n];
        for k in 0..self.n {
            let j = self.q[k];
            for (r, idx) in pattern.column(j) {
                x[r] = values[idx];
            }
            for e in 0..self.u[k].len() {
                let kk = self.u[k][e].0;
                let r = self.prow[kk];
                let xr = x[r];
                self.u[k][e].1 = xr;
                for &(i, lv) in &self.l[kk] {
                    x[i] = x[i] - lv * xr;
                }
                x[r] = T::default();
            }
            let pivot = self.prow[k];
            let d = x[pivot];
            let max =
                self.l[k].iter().map(|&(r, _)| x[r].magnitude()).fold(d.magnitude(), f64::max);
            if d.magnitude() <= 1e-3 * max || d.magnitude() <= 1e-300 || !d.magnitude().is_finite()
            {
                return false;
            }
            self.diag[k] = d;
            x[pivot] = T::default();
            for e in 0..self.l[k].len() {
                let r = self.l[k][e].0;
                self.l[k][e].1 = x[r] / d;
                x[r] = T::default();
            }
        }
        true
    }

    /// Solves `A x = b`, overwriting `b` with `x`.
    pub fn solve(&self, b: &mut [T]) {
        let n = self.n;
        // Forward: L z = P b.
        let mut z = vec![T::default(); n];
        for k in 0..n {
            let zk = b[self.prow[k]];
            z[k] = zk;
            for &(i, lv) in &self.l[k] {
                b[i] = b[i] - lv * zk;
            }
        }
        // Backward: U y = z, column by column.
        for k in (0..n).rev() {
            let yk = z[k] / self.diag[k];
            z[k] = yk;
            for &(kk, uv) in &self.u[k] {
                z[kk] = z[kk] - uv * yk;
            }
        }
        for k in 0..n {
            b[self.q[k]] = z[k];
        }
    }

    /// Nonzeros in `L` and `U`, a measure of fill-in.
    pub fn nnz(&self) -> usize {
        self.n
            + self.l.iter().map(Vec::len).sum::<usize>()
            + self.u.iter().map(Vec::len).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solve::System;

    /// A deterministic pseudo-random sequence.
    fn rng(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed % 10_000) as f64 / 10_000.0
    }

    #[test]
    fn matches_the_dense_solver_on_random_sparse_systems() {
        let mut seed = 0x2545F4914F6CDD1D;
        for n in [1, 2, 5, 30, 120] {
            let mut entries = Vec::new();
            let mut dense = System::new(n);
            for r in 0..n {
                for c in 0..n {
                    // A few off-diagonals, some zero diagonals (like source rows).
                    let keep =
                        if r == c { rng(&mut seed) > 0.2 } else { rng(&mut seed) < 3.0 / n as f64 };
                    if keep {
                        let v = rng(&mut seed) * 2.0 - 1.0;
                        entries.push((r, c, v));
                        dense.a[r * n + c] += v;
                    }
                }
                dense.b[r] = rng(&mut seed);
            }
            // Make it nonsingular: a permutation-like strong entry per row.
            for r in 0..n {
                let c = (r * 7 + 3) % n;
                entries.push((r, c, 5.0));
                dense.a[r * n + c] += 5.0;
            }
            let pattern = Pattern::new(n, entries.iter().map(|e| (e.0, e.1)));
            let mut sys = SparseSystem::new(pattern.clone());
            for &(r, c, v) in &entries {
                sys.a(r, c, v);
            }
            let mut b = dense.b.clone();
            let expected = dense.clone().solve().unwrap();
            let order = minimum_degree(&pattern);
            let lu = Lu::factor(&pattern, &sys.values, &order, 0.1).unwrap();
            lu.solve(&mut b);
            for (x, y) in b.iter().zip(&expected) {
                assert!((x - y).abs() < 1e-8 * (1.0 + y.abs()), "n = {n}: {x} vs {y}");
            }
            // Refactoring with scaled values gives the scaled-inverse solution.
            let mut lu2 = lu.clone();
            let doubled: Vec<f64> = sys.values.iter().map(|v| v * 2.0).collect();
            assert!(lu2.refactor(&pattern, &doubled));
            let mut b2 = dense.b.clone();
            lu2.solve(&mut b2);
            for (x, y) in b2.iter().zip(&expected) {
                assert!((2.0 * x - y).abs() < 1e-8 * (1.0 + y.abs()));
            }
        }
    }

    #[test]
    fn minimum_degree_keeps_a_ladder_free_of_fill() {
        // A resistor ladder is a path: any end-first order eliminates without fill.
        let n = 200;
        let entries: Vec<(usize, usize)> = (1..n).flat_map(|k| [(k - 1, k), (k, k - 1)]).collect();
        let pattern = Pattern::new(n, entries.clone());
        let values: Vec<f64> =
            pattern.entries().map(|(r, c)| if r == c { 2.0 } else { -1.0 }).collect();
        let lu = Lu::factor(&pattern, &values, &minimum_degree(&pattern), 0.1).unwrap();
        assert_eq!(lu.nnz(), pattern.nnz(), "no fill-in");
    }

    #[test]
    fn singular_columns_are_reported() {
        let pattern = Pattern::new(2, [(0, 1)]);
        let values = vec![1.0, 0.0, 0.0]; // (0,0)=1, (0,1)=0, (1,1)=0
        assert!(Lu::factor(&pattern, &values, &[0, 1], 0.1).is_err());
    }
}
