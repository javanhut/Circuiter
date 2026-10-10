//! A plain stroke font for board text: silkscreen references and values, drawn as
//! lines a plotter (or a Gerber file) can trace. Glyphs sit on a 4 × 6 grid, Y down,
//! the baseline at 6; lower case is drawn as upper case.

use crate::layout::P;

fn glyph(c: char) -> &'static str {
    match c.to_ascii_uppercase() {
        '0' => "0,1 1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1; 0,5 4,1",
        '1' => "1,1 2,0 2,6; 1,6 3,6",
        '2' => "0,1 1,0 3,0 4,1 4,2 0,6 4,6",
        '3' => "0,1 1,0 3,0 4,1 4,2 3,3 4,4 4,5 3,6 1,6 0,5; 1,3 3,3",
        '4' => "3,6 3,0 0,4 4,4",
        '5' => "4,0 0,0 0,3 3,3 4,4 4,5 3,6 0,6",
        '6' => "3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,4 3,3 0,3",
        '7' => "0,0 4,0 1,6",
        '8' => "1,3 0,2 0,1 1,0 3,0 4,1 4,2 3,3 1,3 0,4 0,5 1,6 3,6 4,5 4,4 3,3",
        '9' => "4,3 1,3 0,2 0,1 1,0 3,0 4,1 4,5 3,6 1,6",
        'A' => "0,6 0,2 2,0 4,2 4,6; 0,4 4,4",
        'B' => "0,0 0,6 3,6 4,5 4,4 3,3 0,3; 0,0 3,0 4,1 4,2 3,3",
        'C' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5",
        'D' => "0,0 0,6 2,6 4,4 4,2 2,0 0,0",
        'E' => "4,0 0,0 0,6 4,6; 0,3 3,3",
        'F' => "4,0 0,0 0,6; 0,3 3,3",
        'G' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3 2,3",
        'H' => "0,0 0,6; 4,0 4,6; 0,3 4,3",
        'I' => "1,0 3,0; 2,0 2,6; 1,6 3,6",
        'J' => "1,0 4,0; 3,0 3,5 2,6 1,6 0,5",
        'K' => "0,0 0,6; 4,0 0,4; 1,3 4,6",
        'L' => "0,0 0,6 4,6",
        'M' => "0,6 0,0 2,3 4,0 4,6",
        'N' => "0,6 0,0 4,6 4,0",
        'O' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0",
        'P' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3",
        'Q' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0; 2,4 4,6",
        'R' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3; 2,3 4,6",
        'S' => "4,1 3,0 1,0 0,1 0,2 1,3 3,3 4,4 4,5 3,6 1,6 0,5",
        'T' => "0,0 4,0; 2,0 2,6",
        'U' => "0,0 0,5 1,6 3,6 4,5 4,0",
        'V' => "0,0 2,6 4,0",
        'W' => "0,0 1,6 2,3 3,6 4,0",
        'X' => "0,0 4,6; 4,0 0,6",
        'Y' => "0,0 2,3 4,0; 2,3 2,6",
        'Z' => "0,0 4,0 0,6 4,6",
        '+' => "0,3 4,3; 2,1 2,5",
        '-' => "0,3 4,3",
        '.' => "2,5.5 2,6",
        ',' => "2,5 1.5,7",
        '_' => "0,6 4,6",
        '/' => "4,0 0,6",
        ':' => "2,1.5 2,2; 2,4.5 2,5",
        '(' => "3,0 2,1 2,5 3,6",
        ')' => "1,0 2,1 2,5 1,6",
        '#' => "1,0 1,6; 3,0 3,6; 0,2 4,2; 0,4 4,4",
        '=' => "0,2 4,2; 0,4 4,4",
        '*' => "2,1 2,5; 0,2 4,4; 4,2 0,4",
        '%' => "0,6 4,0; 0,0 1,0 1,1 0,1 0,0; 3,5 4,5 4,6 3,6 3,5",
        '!' => "2,0 2,4; 2,5.5 2,6",
        '?' => "0,1 1,0 3,0 4,1 4,2 2,3 2,4; 2,5.5 2,6",
        '\'' => "2,0 2,1.5",
        '"' => "1,0 1,1.5; 3,0 3,1.5",
        '<' => "4,1 0,3 4,5",
        '>' => "0,1 4,3 0,5",
        '[' => "3,0 2,0 2,6 3,6",
        ']' => "1,0 2,0 2,6 1,6",
        'Ω' => "0,6 1,6 1,5 0,3 0,1 1,0 3,0 4,1 4,3 3,5 3,6 4,6",
        'µ' | 'Μ' => "0,7 0,2; 0,4 1,5 3,5 4,4 4,2",
        '°' => "1,0 2,0 2,1 1,1 1,0",
        ' ' => "",
        _ => "0,0 4,0 4,6 0,6 0,0",
    }
}

/// Glyph advance, in grid units.
const ADVANCE: f64 = 5.5;

/// How wide `text` is at cap height `size`.
pub fn width(text: &str, size: f64) -> f64 {
    let n = text.chars().count() as f64;
    if n == 0.0 { 0.0 } else { (n * ADVANCE - 1.5) * size / 6.0 }
}

/// The pen width for text of cap height `size`.
pub fn stroke(size: f64) -> f64 {
    size * 0.15
}

/// The strokes of `text`, centred on `at`, cap height `size`, turned `deg`
/// counter-clockwise on screen and mirrored (for the back of the board) if asked.
pub fn strokes(text: &str, at: P, size: f64, deg: f64, mirror: bool) -> Vec<Vec<P>> {
    let k = size / 6.0;
    let (w, h) = (width(text, size), size);
    let (s, c) = deg.to_radians().sin_cos();
    let mut out = Vec::new();
    for (n, ch) in text.chars().enumerate() {
        let x0 = n as f64 * ADVANCE * k - w / 2.0;
        for stroke in glyph(ch).split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let line: Vec<P> = stroke
                .split_whitespace()
                .filter_map(|xy| {
                    let (x, y) = xy.split_once(',')?;
                    let (x, y) = (x.parse::<f64>().ok()?, y.parse::<f64>().ok()?);
                    let mut px = x0 + x * k;
                    let py = y * k - h / 2.0;
                    if mirror {
                        px = -px;
                    }
                    Some([at[0] + px * c + py * s, at[1] - px * s + py * c])
                })
                .collect();
            if line.len() >= 2 {
                out.push(line);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_centred_and_sized() {
        // "RE": both glyphs fill their cells, so the ink spans the whole width.
        let lines = strokes("RE", [10.0, 5.0], 1.2, 0.0, false);
        let xs = lines.iter().flatten().map(|p| p[0]);
        let ys = lines.iter().flatten().map(|p| p[1]);
        let (lo, hi) = (xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max));
        let (top, bottom) = (ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max));
        assert!(((lo + hi) / 2.0 - 10.0).abs() < 1e-9, "{lo} {hi}");
        assert!((bottom - top - 1.2).abs() < 1e-9 && ((top + bottom) / 2.0 - 5.0).abs() < 1e-9);
        assert!((hi - lo - width("RE", 1.2)).abs() < 1e-9);
        // Every printable ASCII character has a glyph of its own.
        for c in (b'!'..=b'~').map(char::from).filter(|c| !"$&@\\^`{|}~;".contains(*c)) {
            assert_ne!(glyph(c), glyph('\u{1}'), "{c}");
        }
    }
}
