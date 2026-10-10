//! Outlines from 2D drawings: DXF (lines, polylines with arcs, circles, arcs) and SVG
//! (rectangles, circles, ellipses, polygons, polylines and paths, curves flattened), in
//! millimetres. Pieces are joined end to end into closed outlines, largest first, as
//! sketches to extrude or a board's edge.

use circuiter_model::model3d::{Sketch, SketchPoint};

/// A closed outline: corners with the bulge of the edge leaving each (0 for straight).
pub type Loop = Vec<(f64, f64, f64)>;

/// A piece of a drawing: from, to, bulge.
type Piece = ((f64, f64), (f64, f64), f64);

/// An old-style POLYLINE being read: whether it closes, and its vertices so far.
type Open = Option<(bool, Vec<(f64, f64, f64)>)>;

/// A path's subpaths: points, and whether each closes.
type Subpaths = Vec<(Vec<(f64, f64)>, bool)>;

/// The outlines in a drawing, by its file name's extension (`dxf` or `svg`).
pub fn outlines(name: &str, text: &str) -> Result<Vec<Loop>, String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mut loops = match ext.as_str() {
        "dxf" => dxf(text)?,
        "svg" => svg(text)?,
        other => return Err(format!("can't read .{other} drawings; use DXF or SVG")),
    };
    loops.retain(|l| l.len() >= 2);
    if loops.is_empty() {
        return Err("no closed outlines in the drawing".into());
    }
    loops.sort_by(|a, b| area(b).abs().total_cmp(&area(a).abs()));
    Ok(loops)
}

/// The area inside a loop (its arcs counted as their chords).
pub fn area(l: &Loop) -> f64 {
    let n = l.len();
    (0..n).map(|k| l[k].0 * l[(k + 1) % n].1 - l[(k + 1) % n].0 * l[k].1).sum::<f64>() / 2.0
}

/// A loop as a sketch, moved so the largest outline's box is centred on the origin by
/// `shift`, and with Y flipped if `flip` (drawings with Y down).
pub fn sketch(l: &Loop, shift: (f64, f64)) -> Sketch {
    let f = |v: f64| {
        let s = format!("{v:.4}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" { "0".to_owned() } else { s.to_owned() }
    };
    Sketch {
        points: l
            .iter()
            .map(|&(x, y, b)| SketchPoint {
                x: f(x - shift.0),
                y: f(y - shift.1),
                bulge: if b.abs() < 1e-9 { String::new() } else { f(b) },
            })
            .collect(),
    }
}

/// The middle of a loop's box.
pub fn centre(l: &Loop) -> (f64, f64) {
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for &(x, y, _) in l {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0)
}

/// A loop's corners with its arcs drawn as chords every few degrees.
pub fn flatten(l: &Loop) -> Vec<(f64, f64)> {
    let n = l.len();
    let mut out = Vec::new();
    for k in 0..n {
        let (x0, y0, b) = l[k];
        let (x1, y1, _) = l[(k + 1) % n];
        out.push((x0, y0));
        if b.abs() > 1e-9 {
            // The arc's centre and sweep from its bulge.
            let sweep = 4.0 * b.atan();
            let (dx, dy) = (x1 - x0, y1 - y0);
            let chord = dx.hypot(dy);
            if chord < 1e-12 {
                continue;
            }
            let r = chord / (2.0 * (sweep / 2.0).sin()).abs();
            let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let h = (r * r - chord * chord / 4.0).max(0.0).sqrt()
                * if b.abs() > 1.0 { -1.0 } else { 1.0 };
            let s = b.signum();
            let c = (mid.0 - s * h * dy / chord, mid.1 + s * h * dx / chord);
            let a0 = (y0 - c.1).atan2(x0 - c.0);
            let steps = ((sweep.abs().to_degrees() / 10.0).ceil() as usize).max(2);
            for j in 1..steps {
                let a = a0 + sweep * j as f64 / steps as f64;
                out.push((c.0 + r * a.cos(), c.1 + r * a.sin()));
            }
        }
    }
    out
}

/// Joins pieces end to end into loops, within 1 µm.
fn chain(mut pieces: Vec<Piece>) -> Vec<Loop> {
    const TOL: f64 = 1e-3;
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < TOL && (a.1 - b.1).abs() < TOL;
    let mut loops = Vec::new();
    while let Some(first) = pieces.pop() {
        let start = first.0;
        let mut l: Loop = vec![(first.0.0, first.0.1, first.2)];
        let mut end = first.1;
        loop {
            if near(end, start) {
                loops.push(l);
                break;
            }
            let next = pieces.iter().position(|p| near(p.0, end) || near(p.1, end));
            let Some(k) = next else { break };
            let p = pieces.swap_remove(k);
            if near(p.0, end) {
                l.push((p.0.0, p.0.1, p.2));
                end = p.1;
            } else {
                // Walked backwards: the arc bends the other way.
                l.push((p.1.0, p.1.1, -p.2));
                end = p.0;
            }
        }
    }
    loops
}

/// DXF: group codes and values in pairs, entities in the ENTITIES section.
fn dxf(text: &str) -> Result<Vec<Loop>, String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    if lines.len() < 2 {
        return Err("an empty DXF file".into());
    }
    let pairs: Vec<(i32, &str)> =
        lines.chunks_exact(2).filter_map(|c| Some((c[0].parse::<i32>().ok()?, c[1]))).collect();
    // Units: $INSUNITS 1 inches, 4 millimetres (the default here), 5 centimetres.
    let mut scale = 1.0;
    for w in pairs.windows(2) {
        if w[0] == (9, "$INSUNITS") {
            scale = match w[1].1.trim() {
                "1" => 25.4,
                "5" => 10.0,
                "6" => 1000.0,
                _ => 1.0,
            };
        }
    }
    let num =
        |v: &str| v.trim().parse::<f64>().map_err(|_| format!("`{v}` isn't a number in the DXF"));
    let mut pieces: Vec<Piece> = Vec::new();
    let mut loops = Vec::new();
    // Each entity: its type and its group pairs.
    let mut entities: Vec<(&str, Vec<(i32, &str)>)> = Vec::new();
    let mut in_entities = false;
    let mut k = 0;
    while k < pairs.len() {
        let (code, value) = pairs[k];
        if code == 2 && value == "ENTITIES" {
            in_entities = true;
        } else if code == 0 && value == "ENDSEC" {
            in_entities = false;
        } else if in_entities && code == 0 {
            entities.push((value, Vec::new()));
        } else if in_entities {
            if let Some(e) = entities.last_mut() {
                e.1.push((code, value));
            }
        }
        k += 1;
    }
    let mut polyline: Open = None;
    for (kind, groups) in &entities {
        let get = |c: i32| groups.iter().find(|g| g.0 == c).map(|g| g.1);
        let val =
            |c: i32| -> Result<f64, String> { get(c).map_or(Ok(0.0), num).map(|v| v * scale) };
        match *kind {
            "LINE" => pieces.push(((val(10)?, val(20)?), (val(11)?, val(21)?), 0.0)),
            "CIRCLE" => {
                let (x, y, r) = (val(10)?, val(20)?, val(40)?);
                loops.push(vec![(x - r, y, 1.0), (x + r, y, 1.0)]);
            }
            "ARC" => {
                let (x, y, r) = (val(10)?, val(20)?, val(40)?);
                let (a0, a1) = (
                    get(50).map_or(Ok(0.0), num)?.to_radians(),
                    get(51).map_or(Ok(0.0), num)?.to_radians(),
                );
                let mut sweep = a1 - a0;
                if sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let p = |a: f64| (x + r * a.cos(), y + r * a.sin());
                pieces.push((p(a0), p(a1), (sweep / 4.0).tan()));
            }
            "LWPOLYLINE" => {
                let closed =
                    get(70).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0) & 1 == 1;
                let mut pts: Vec<(f64, f64, f64)> = Vec::new();
                for &(c, v) in groups {
                    match c {
                        10 => pts.push((num(v)? * scale, 0.0, 0.0)),
                        20 => {
                            if let Some(p) = pts.last_mut() {
                                p.1 = num(v)? * scale;
                            }
                        }
                        42 => {
                            if let Some(p) = pts.last_mut() {
                                p.2 = num(v)?;
                            }
                        }
                        _ => {}
                    }
                }
                add_polyline(&mut loops, &mut pieces, pts, closed);
            }
            "POLYLINE" => {
                let closed =
                    get(70).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0) & 1 == 1;
                polyline = Some((closed, Vec::new()));
            }
            "VERTEX" => {
                if let Some((_, pts)) = &mut polyline {
                    pts.push((val(10)?, val(20)?, get(42).map_or(Ok(0.0), num)?));
                }
            }
            "SEQEND" => {
                if let Some((closed, pts)) = polyline.take() {
                    add_polyline(&mut loops, &mut pieces, pts, closed);
                }
            }
            _ => {}
        }
    }
    loops.extend(chain(pieces));
    Ok(loops)
}

fn add_polyline(
    loops: &mut Vec<Loop>,
    pieces: &mut Vec<Piece>,
    pts: Vec<(f64, f64, f64)>,
    closed: bool,
) {
    if pts.len() < 2 {
        return;
    }
    let first = (pts[0].0, pts[0].1);
    let last = (pts[pts.len() - 1].0, pts[pts.len() - 1].1);
    if closed || ((first.0 - last.0).abs() < 1e-6 && (first.1 - last.1).abs() < 1e-6) {
        let mut l = pts;
        if !closed {
            l.pop();
        }
        loops.push(l);
    } else {
        for w in pts.windows(2) {
            pieces.push(((w[0].0, w[0].1), (w[1].0, w[1].1), w[0].2));
        }
    }
}

/// An attribute's value in an SVG tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(k) = rest.find(name) {
        let before = rest[..k].chars().last();
        let after = &rest[k + name.len()..];
        let after_trim = after.trim_start();
        if before.is_none_or(|c| c.is_whitespace()) && after_trim.starts_with('=') {
            let v = after_trim[1..].trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let end = v[1..].find(q)?;
                return Some(&v[1..1 + end]);
            }
        }
        rest = after;
    }
    None
}

/// A length in an SVG attribute, in millimetres (plain numbers are CSS pixels).
fn length(v: &str) -> Option<f64> {
    let v = v.trim();
    let (num, unit) =
        v.split_at(v.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(v.len()));
    let n: f64 = num.trim().parse().ok()?;
    Some(match unit {
        "mm" => n,
        "cm" => n * 10.0,
        "in" => n * 25.4,
        "pt" => n * 25.4 / 72.0,
        _ => n * 25.4 / 96.0,
    })
}

/// SVG: shapes in user units, scaled to millimetres by the root's width and viewBox
/// (96 to the inch without them). Y runs down in SVG; outlines come back with Y up.
fn svg(text: &str) -> Result<Vec<Loop>, String> {
    let root = text
        .find("<svg")
        .map(|k| &text[k..text[k..].find('>').map_or(text.len(), |e| k + e)])
        .ok_or("not an SVG file")?;
    let viewbox: Option<Vec<f64>> = attr(root, "viewBox").map(|v| {
        v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|s| s.parse().ok()).collect()
    });
    let scale =
        match (attr(root, "width").and_then(length), viewbox.as_ref().filter(|v| v.len() == 4)) {
            (Some(w), Some(vb)) if vb[2] > 0.0 => w / vb[2],
            _ => 25.4 / 96.0,
        };
    let mut loops: Vec<Loop> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let tf = |x: f64, y: f64| (x * scale, -y * scale);
    let mut rest = text;
    while let Some(k) = rest.find('<') {
        rest = &rest[k + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        let f = |a: &str| attr(tag, a).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        match name {
            "rect" => {
                let (x, y, w, h) = (f("x"), f("y"), f("width"), f("height"));
                if w > 0.0 && h > 0.0 {
                    loops.push(
                        [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
                            .iter()
                            .map(|&(a, b)| {
                                let p = tf(a, b);
                                (p.0, p.1, 0.0)
                            })
                            .collect(),
                    );
                }
            }
            "circle" => {
                let (cx, cy, r) = (f("cx"), f("cy"), f("r"));
                let (a, b) = (tf(cx - r, cy), tf(cx + r, cy));
                loops.push(vec![(a.0, a.1, 1.0), (b.0, b.1, 1.0)]);
            }
            "ellipse" => {
                let (cx, cy, rx, ry) = (f("cx"), f("cy"), f("rx"), f("ry"));
                loops.push(
                    (0..48)
                        .map(|j| {
                            let a = std::f64::consts::TAU * j as f64 / 48.0;
                            let p = tf(cx + rx * a.cos(), cy + ry * a.sin());
                            (p.0, p.1, 0.0)
                        })
                        .collect(),
                );
            }
            "polygon" | "polyline" => {
                let nums: Vec<f64> = attr(tag, "points")
                    .unwrap_or("")
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter_map(|s| s.parse().ok())
                    .collect();
                let pts: Vec<(f64, f64, f64)> = nums
                    .chunks_exact(2)
                    .map(|c| {
                        let p = tf(c[0], c[1]);
                        (p.0, p.1, 0.0)
                    })
                    .collect();
                add_polyline(&mut loops, &mut pieces, pts, name == "polygon");
            }
            "path" => {
                for sub in path(attr(tag, "d").unwrap_or(""))? {
                    let pts: Vec<(f64, f64, f64)> = sub
                        .0
                        .iter()
                        .map(|&(x, y)| {
                            let p = tf(x, y);
                            (p.0, p.1, 0.0)
                        })
                        .collect();
                    add_polyline(&mut loops, &mut pieces, pts, sub.1);
                }
            }
            _ => {}
        }
    }
    loops.extend(chain(pieces));
    Ok(loops)
}

/// An SVG path's subpaths as points (curves flattened), and whether each closes.
fn path(d: &str) -> Result<Subpaths, String> {
    // Tokens: commands and numbers.
    let mut tokens: Vec<Result<char, f64>> = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, tokens: &mut Vec<Result<char, f64>>| -> Result<(), String> {
        if !num.is_empty() {
            tokens.push(Err(num
                .parse::<f64>()
                .map_err(|_| format!("bad number `{num}` in a path"))?));
            num.clear();
        }
        Ok(())
    };
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            flush(&mut num, &mut tokens)?;
            tokens.push(Ok(c));
        } else if c == ',' || c.is_whitespace() {
            flush(&mut num, &mut tokens)?;
        } else if (c == '-' && !num.is_empty() && !num.ends_with(['e', 'E']))
            || (c == '.' && num.contains('.') && !num.contains(['e', 'E']))
        {
            // A sign or a second point starts the next number: `1-2`, `.5.5`.
            flush(&mut num, &mut tokens)?;
            num.push(c);
        } else {
            num.push(c);
        }
    }
    flush(&mut num, &mut tokens)?;
    let mut out: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
    let mut cur = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut cmd = 'M';
    let mut k = 0;
    let mut last_ctrl: Option<(f64, f64)> = None;
    let take = |k: &mut usize, n: usize| -> Option<Vec<f64>> {
        let v: Vec<f64> = tokens
            .get(*k..*k + n)?
            .iter()
            .map(|t| t.as_ref().err().copied())
            .collect::<Option<_>>()?;
        *k += n;
        Some(v)
    };
    let end_sub =
        |pts: &mut Vec<(f64, f64)>, out: &mut Vec<(Vec<(f64, f64)>, bool)>, closed: bool| {
            if pts.len() >= 2 {
                out.push((std::mem::take(pts), closed));
            } else {
                pts.clear();
            }
        };
    while k < tokens.len() {
        if let Ok(c) = tokens[k] {
            cmd = c;
            k += 1;
            if c == 'Z' || c == 'z' {
                end_sub(&mut pts, &mut out, true);
                cur = start;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        let n = match cmd.to_ascii_uppercase() {
            'M' | 'L' | 'T' => 2,
            'H' | 'V' => 1,
            'C' => 6,
            'S' | 'Q' => 4,
            'A' => 7,
            _ => return Err(format!("path command `{cmd}` isn't understood")),
        };
        let Some(v) = take(&mut k, n) else { break };
        match cmd.to_ascii_uppercase() {
            'M' => {
                end_sub(&mut pts, &mut out, false);
                cur = (base.0 + v[0], base.1 + v[1]);
                start = cur;
                pts.push(cur);
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' | 'T' => {
                cur = (base.0 + v[0], base.1 + v[1]);
                pts.push(cur);
            }
            'H' => {
                cur = (if rel { cur.0 + v[0] } else { v[0] }, cur.1);
                pts.push(cur);
            }
            'V' => {
                cur = (cur.0, if rel { cur.1 + v[0] } else { v[0] });
                pts.push(cur);
            }
            'C' | 'S' | 'Q' => {
                let (c1, c2, to) = match cmd.to_ascii_uppercase() {
                    'C' => (
                        (base.0 + v[0], base.1 + v[1]),
                        (base.0 + v[2], base.1 + v[3]),
                        (base.0 + v[4], base.1 + v[5]),
                    ),
                    'S' => {
                        let c1 = last_ctrl.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                        (c1, (base.0 + v[0], base.1 + v[1]), (base.0 + v[2], base.1 + v[3]))
                    }
                    _ => {
                        // A quadratic as a cubic.
                        let q = (base.0 + v[0], base.1 + v[1]);
                        let to = (base.0 + v[2], base.1 + v[3]);
                        (
                            (cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1)),
                            (to.0 + 2.0 / 3.0 * (q.0 - to.0), to.1 + 2.0 / 3.0 * (q.1 - to.1)),
                            to,
                        )
                    }
                };
                for j in 1..=16 {
                    let t = j as f64 / 16.0;
                    let u = 1.0 - t;
                    let x = u * u * u * cur.0
                        + 3.0 * u * u * t * c1.0
                        + 3.0 * u * t * t * c2.0
                        + t * t * t * to.0;
                    let y = u * u * u * cur.1
                        + 3.0 * u * u * t * c1.1
                        + 3.0 * u * t * t * c2.1
                        + t * t * t * to.1;
                    pts.push((x, y));
                }
                last_ctrl = Some(c2);
                cur = to;
                continue;
            }
            'A' => {
                // Elliptical arcs as their chord: fine for the straight-sided outlines
                // most drawings are; flattened properly below when circular.
                let to = (base.0 + v[5], base.1 + v[6]);
                let (rx, ry) = (v[0].abs(), v[1].abs());
                if (rx - ry).abs() < 1e-9 && rx > 0.0 {
                    let chord = (to.0 - cur.0).hypot(to.1 - cur.1);
                    let r = rx.max(chord / 2.0);
                    let mut sweep = 2.0 * (chord / (2.0 * r)).clamp(-1.0, 1.0).asin();
                    if v[3] != 0.0 {
                        sweep = std::f64::consts::TAU - sweep;
                    }
                    let sign = if v[4] != 0.0 { 1.0 } else { -1.0 };
                    // The centre, on the side the flags pick.
                    let mid = ((cur.0 + to.0) / 2.0, (cur.1 + to.1) / 2.0);
                    let h = (r * r - chord * chord / 4.0).max(0.0).sqrt();
                    let (dx, dy) =
                        ((to.0 - cur.0) / chord.max(1e-12), (to.1 - cur.1) / chord.max(1e-12));
                    let side = if (v[3] != 0.0) == (v[4] != 0.0) { -1.0 } else { 1.0 };
                    let c = (mid.0 - side * h * dy, mid.1 + side * h * dx);
                    let a0 = (cur.1 - c.1).atan2(cur.0 - c.0);
                    let steps = ((sweep.abs().to_degrees() / 10.0).ceil() as usize).max(2);
                    for j in 1..steps {
                        let a = a0 + sign * sweep * j as f64 / steps as f64;
                        pts.push((c.0 + r * a.cos(), c.1 + r * a.sin()));
                    }
                }
                cur = to;
                pts.push(cur);
            }
            _ => {}
        }
        last_ctrl = None;
    }
    end_sub(&mut pts, &mut out, false);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxf_outlines_are_joined_and_scaled() {
        // A 40 × 20 mm plate drawn as four LINEs, with a 3 mm hole, in millimetres.
        let mut dxf = String::from(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n",
        );
        for (a, b) in
            [((0, 0), (40, 0)), ((40, 20), (40, 0)), ((40, 20), (0, 20)), ((0, 20), (0, 0))]
        {
            dxf.push_str(&format!(
                "0\nLINE\n8\n0\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n",
                a.0, a.1, b.0, b.1
            ));
        }
        dxf.push_str("0\nCIRCLE\n8\n0\n10\n10\n20\n10\n40\n1.5\n0\nENDSEC\n0\nEOF\n");
        let loops = outlines("plate.dxf", &dxf).unwrap();
        assert_eq!(loops.len(), 2);
        assert!((area(&loops[0]).abs() - 800.0).abs() < 1e-9, "the plate comes first");
        // The circle is two half arcs.
        assert_eq!(loops[1].len(), 2);
        let flat = flatten(&loops[1]);
        let a: f64 = (0..flat.len())
            .map(|k| {
                flat[k].0 * flat[(k + 1) % flat.len()].1 - flat[(k + 1) % flat.len()].0 * flat[k].1
            })
            .sum::<f64>()
            / 2.0;
        assert!((a.abs() - std::f64::consts::PI * 2.25).abs() < 0.1, "{a}");
        // In inches, ×25.4.
        let inch = dxf.replace("$INSUNITS\n70\n4", "$INSUNITS\n70\n1");
        let loops = outlines("plate.dxf", &inch).unwrap();
        assert!((area(&loops[0]).abs() - 800.0 * 25.4 * 25.4).abs() < 1e-3);
        // An LWPOLYLINE with a bulge: a slot.
        let slot = "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n90\n4\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n42\n1\n10\n10\n20\n4\n10\n0\n20\n4\n42\n1\n0\nENDSEC\n0\nEOF\n";
        let loops = outlines("slot.dxf", slot).unwrap();
        assert_eq!(loops[0].len(), 4);
        assert_eq!(loops[0][1].2, 1.0);
    }

    #[test]
    fn svg_shapes_and_paths_become_outlines() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 200 100">
            <rect x="0" y="0" width="200" height="100"/>
            <circle cx="20" cy="20" r="5"/>
            <path d="M 50 50 h 20 v 20 h -20 z"/>
            <path d="M100,10 C120,10 120,30 100,30 Z"/>
        </svg>"#;
        let loops = outlines("panel.svg", svg).unwrap();
        assert_eq!(loops.len(), 4);
        // 200 × 100 user units at 0.5 mm each: 100 × 50 mm.
        assert!((area(&loops[0]).abs() - 5000.0).abs() < 1e-6);
        // The square path: 20 units = 10 mm on a side.
        assert!(loops.iter().any(|l| (area(l).abs() - 100.0).abs() < 1e-6));
        // Y comes back up: the panel's top edge at y = 0, its bottom at −50.
        let ys: Vec<f64> = loops[0].iter().map(|p| p.1).collect();
        assert!(ys.iter().any(|y| (*y + 50.0).abs() < 1e-9));
    }
}
