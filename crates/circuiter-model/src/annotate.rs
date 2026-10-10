//! Annotation, as KiCad's: giving every part on a sheet its reference designator, its
//! cell's prefix and a number (`R1`, `R2`, `U1`), numbered across the sheet in reading
//! order. Parts already numbered keep their numbers unless asked to start over; parts
//! without one (`R`, `R?`) or sharing one get the next free number.

use crate::board::Board;
use crate::design::Design;
use crate::schematic::Schematic;
use std::collections::{BTreeMap, BTreeSet};

/// Which way numbers run across the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// Left to right along rows, rows top to bottom.
    #[default]
    Rows,
    /// Top to bottom down columns, columns left to right.
    Columns,
}

/// The number in `name` after `prefix`, if it is one: `R12` → 12.
fn number(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok()).filter(|&n| n > 0)
}

/// Renumbers the parts of `schematic` (a sheet in `library`) and returns what was
/// renamed, old name to new. With `reset`, every part is numbered afresh.
pub fn annotate(
    design: &Design,
    library: &str,
    schematic: &mut Schematic,
    order: Order,
    reset: bool,
) -> BTreeMap<String, String> {
    // Each part's prefix and where it is.
    let mut parts: Vec<(usize, String, (i64, i64))> = Vec::new();
    for (k, inst) in schematic.instances.iter().enumerate() {
        let prefix = design
            .resolve(&inst.cell, library)
            .map_or_else(|| "X".to_owned(), |(_, d)| d.cell.prefix.clone());
        let p = inst.placement.position;
        parts.push((k, prefix, (p.x.nm(), p.y.nm())));
    }
    parts.sort_by_key(|(_, _, (x, y))| match order {
        Order::Rows => (*y, *x),
        Order::Columns => (*x, *y),
    });
    // Numbers kept, by prefix; the first part to hold a number keeps it.
    let mut taken: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut keep = vec![false; schematic.instances.len()];
    if !reset {
        for (k, prefix, _) in &parts {
            if let Some(n) = number(&schematic.instances[*k].name, prefix) {
                if taken.entry(prefix.clone()).or_default().insert(n) {
                    keep[*k] = true;
                }
            }
        }
    }
    let mut renames = BTreeMap::new();
    let mut new_names: Vec<String> = schematic.instances.iter().map(|i| i.name.clone()).collect();
    for (k, prefix, _) in &parts {
        if keep[*k] {
            continue;
        }
        let used = taken.entry(prefix.clone()).or_default();
        let n = (1..).find(|n| !used.contains(n)).expect("a free number");
        used.insert(n);
        new_names[*k] = format!("{prefix}{n}");
    }
    // Names must stay unique among the sheet's parts as a whole: a part keeping `U1`
    // under one prefix mustn't collide with another's new name.
    for (k, inst) in schematic.instances.iter_mut().enumerate() {
        if inst.name != new_names[k] {
            renames.insert(inst.name.clone(), new_names[k].clone());
            inst.name = new_names[k].clone();
        }
    }
    renames
}

/// Keeps a board's footprints with their parts after `renames` (from [`annotate`]):
/// a footprint is keyed by its part's instance path.
pub fn rename_on_board(board: &mut Board, renames: &BTreeMap<String, String>) {
    for f in &mut board.footprints {
        let (head, rest) = match f.path.split_once('.') {
            Some((h, r)) => (h.to_owned(), Some(r.to_owned())),
            None => (f.path.clone(), None),
        };
        if let Some(new) = renames.get(&head) {
            f.path = match rest {
                Some(r) => format!("{new}.{r}"),
                None => new.clone(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Placed;
    use crate::cell::{Cell, CellDef};
    use crate::geom::{Point, Transform};
    use crate::library::Library;
    use crate::schematic::Instance;

    fn design() -> Design {
        let mut lib = Library::new("t");
        for (name, prefix) in [("res", "R"), ("cap", "C")] {
            let cell = Cell { prefix: prefix.into(), ..Cell::default() };
            lib.insert(CellDef { name: name.into(), cell, ..CellDef::default() });
        }
        let mut d = Design::new();
        d.add_library(lib).unwrap();
        d
    }

    fn part(cell: &str, name: &str, x: f64, y: f64) -> Instance {
        Instance {
            name: name.into(),
            cell: cell.parse().unwrap(),
            placement: Transform::at(Point::mm(x, y)),
            params: Default::default(),
            view: None,
            fields: Default::default(),
        }
    }

    #[test]
    fn parts_are_numbered_in_reading_order() {
        let d = design();
        let mut s = Schematic {
            instances: vec![
                part("res", "R?", 30.0, 0.0),
                part("res", "R5", 0.0, 10.0),
                part("cap", "C", 10.0, 0.0),
                part("res", "R", 0.0, 0.0),
                part("res", "R5", 50.0, 50.0),
            ],
            ..Schematic::default()
        };
        let renames = annotate(&d, "t", &mut s, Order::Rows, false);
        let names: Vec<&str> = s.instances.iter().map(|i| i.name.as_str()).collect();
        // R5 keeps its number (the first one read); the rest fill in from 1.
        assert_eq!(names, ["R2", "R5", "C1", "R1", "R3"]);
        assert_eq!(renames.get("R?").map(String::as_str), Some("R2"));
        // Starting over numbers everything by position.
        annotate(&d, "t", &mut s, Order::Rows, true);
        let names: Vec<&str> = s.instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["R2", "R3", "C1", "R1", "R4"]);
        annotate(&d, "t", &mut s, Order::Columns, true);
        let names: Vec<&str> = s.instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["R3", "R2", "C1", "R1", "R4"]);
    }

    #[test]
    fn footprints_follow_their_parts() {
        let mut b = Board::new(10.0, 10.0);
        for path in ["R1", "X1.R1", "R10"] {
            b.footprints.push(Placed {
                path: path.into(),
                at: Point::mm(0.0, 0.0),
                rotation: 0.0,
                back: false,
                locked: false,
            });
        }
        let renames = BTreeMap::from([
            ("R1".to_owned(), "R2".to_owned()),
            ("X1".to_owned(), "X3".to_owned()),
        ]);
        rename_on_board(&mut b, &renames);
        let paths: Vec<&str> = b.footprints.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["R2", "X3.R1", "R10"]);
    }
}
