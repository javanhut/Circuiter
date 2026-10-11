//! Controlled impedance from the stack-up, by IPC-2141's closed-form approximations
//! (as Altium's older Impedance Formula Editor uses; its field solver is more exact,
//! and the formulas can be several per cent out, so check against the board house):
//! a track on an outer layer is a microstrip over the plane below, one on an inner
//! layer a stripline between the planes either side; pairs are edge-coupled.

use circuiter_model::board::{ImpedanceProfile, Layer, StackKind, Stackup};

/// A track's surroundings: microstrip `h` over its plane, or stripline `b` between
/// two; εr of the dielectric; copper thickness `t` (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry {
    Microstrip { h: f64, er: f64, t: f64 },
    Stripline { b: f64, er: f64, t: f64 },
}

/// Single-ended impedance of a track `w` wide, ohms.
pub fn single(g: Geometry, w: f64) -> f64 {
    match g {
        Geometry::Microstrip { h, er, t } => {
            87.0 / (er + 1.41).sqrt() * (5.98 * h / (0.8 * w + t)).ln()
        }
        Geometry::Stripline { b, er, t } => {
            60.0 / er.sqrt() * (4.0 * b / (0.67 * std::f64::consts::PI * (0.8 * w + t))).ln()
        }
    }
}

/// Differential impedance of an edge-coupled pair, tracks `w` wide `s` apart, ohms.
pub fn differential(g: Geometry, w: f64, s: f64) -> f64 {
    let z0 = single(g, w);
    match g {
        Geometry::Microstrip { h, .. } => 2.0 * z0 * (1.0 - 0.48 * (-0.96 * s / h).exp()),
        Geometry::Stripline { b, .. } => 2.0 * z0 * (1.0 - 0.347 * (-2.9 * s / b).exp()),
    }
}

/// The track width giving `ohms` (single, or differential with `gap`), by bisection:
/// impedance falls as width grows. `None` if it can't be reached between 25 µm and
/// 10 mm.
pub fn width_for(g: Geometry, ohms: f64, gap: Option<f64>) -> Option<f64> {
    let z = |w: f64| gap.map_or_else(|| single(g, w), |s| differential(g, w, s));
    let (mut lo, mut hi) = (0.025, 10.0);
    if !(z(hi)..=z(lo)).contains(&ohms) {
        return None;
    }
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        if z(mid) > ohms {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// The surroundings of a track on copper `layer` in `stack`.
pub fn geometry(stack: &Stackup, layer: Layer) -> Option<Geometry> {
    let coppers: Vec<usize> = stack
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind == StackKind::Copper)
        .map(|(k, _)| k)
        .collect();
    let n = coppers.len();
    let index = match layer {
        Layer::FCu => 0,
        Layer::BCu => n.checked_sub(1)?,
        Layer::In(k) => k as usize,
        _ => return None,
    };
    let at = *coppers.get(index)?;
    let t = stack.layers[at].thickness.mm();
    // Dielectric between copper `a` and copper `b` (stack indices): thickness, and εr
    // weighted by thickness.
    let between = |a: usize, b: usize| {
        let (lo, hi) = (a.min(b), a.max(b));
        let d: Vec<_> =
            stack.layers[lo + 1..hi].iter().filter(|l| l.kind != StackKind::Copper).collect();
        let h: f64 = d.iter().map(|l| l.thickness.mm()).sum();
        let er =
            if h > 0.0 { d.iter().map(|l| l.er * l.thickness.mm()).sum::<f64>() / h } else { 4.5 };
        (h, er)
    };
    if index == 0 || index == n - 1 {
        let plane = if index == 0 { *coppers.get(1)? } else { coppers[n - 2] };
        let (h, er) = between(at, plane);
        Some(Geometry::Microstrip { h, er, t })
    } else {
        let (above, below) = (coppers[index - 1], coppers[index + 1]);
        let (h1, e1) = between(above, at);
        let (h2, e2) = between(at, below);
        Some(Geometry::Stripline { b: h1 + h2 + t, er: (e1 * h1 + e2 * h2) / (h1 + h2), t })
    }
}

/// The width a profile asks for on its stack, mm.
pub fn profile_width(stack: &Stackup, profile: &ImpedanceProfile) -> Option<f64> {
    let g = geometry(stack, profile.layer)?;
    width_for(g, profile.ohms, profile.differential.then(|| profile.gap.mm()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_for_impedances() {
        // 50 Ω on a 1.6 mm two-layer FR-4 board is a track about 3 mm wide.
        let stack = Stackup::standard(2, 1.6);
        let g = geometry(&stack, Layer::FCu).unwrap();
        let Geometry::Microstrip { h, er, .. } = g else { panic!("{g:?}") };
        assert!((h - 1.5).abs() < 0.05 && (er - 4.5).abs() < 1e-9, "{g:?}");
        let w = width_for(g, 50.0, None).unwrap();
        assert!((2.5..3.3).contains(&w), "{w}");
        assert!((single(g, w) - 50.0).abs() < 1e-6);
        // Narrower tracks, higher impedance.
        assert!(single(g, 0.5) > single(g, 1.0));
        // On a four-layer board the outer layers sit close to their planes: a 50 Ω track
        // is a few tenths of a millimetre.
        let four = Stackup::standard(4, 1.6);
        let w4 = width_for(geometry(&four, Layer::FCu).unwrap(), 50.0, None).unwrap();
        assert!((0.2..0.5).contains(&w4), "{w4}");
        // Inner layers are striplines.
        assert!(matches!(geometry(&four, Layer::In(1)).unwrap(), Geometry::Stripline { .. }));
        // A 100 Ω pair.
        let pair = width_for(geometry(&four, Layer::FCu).unwrap(), 100.0, Some(0.2)).unwrap();
        assert!(pair > 0.05 && pair < w4 * 1.2, "{pair}");
        assert!((four.thickness() - 1.6).abs() < 0.01, "{}", four.thickness());
    }
}
