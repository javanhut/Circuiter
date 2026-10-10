//! Bodies placed together: one part, or every part on a sheet, set where its symbol sits
//! (until boards are laid out, a sheet's 3D view is a preview of its parts on a board
//! the size of the sheet's drawing).

use crate::math::{Mat4, V3};
use crate::{Mesh, color, cuboid, packages};
use circuiter_model::geom::{Point, Transform};
use circuiter_model::{CellDef, CellId, Design};
use std::collections::BTreeMap;

/// Board thickness under a sheet's parts.
pub const BOARD: f32 = 1.6;

/// The body of `def` with these instance parameter values (missing ones take the
/// cell's defaults), or `None` if the cell has no 3D view.
pub fn body(def: &CellDef, overrides: &BTreeMap<String, String>) -> Result<Option<Mesh>, String> {
    let Some(view) = &def.model3d else { return Ok(None) };
    let param = |name: &str| -> Option<String> {
        overrides.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone())
    };
    if !view.features.is_empty() {
        // Feature dimensions name the parameters directly, as numbers.
        let vars = |name: &str| -> Option<f64> {
            circuiter_model::units::parse_quantity(&param(name)?, None).ok()
        };
        let built = crate::cad::evaluate(&view.features, &vars);
        if let (true, Some((k, e))) = (built.mesh.vertices.is_empty(), built.errors.first()) {
            return Err(format!("{}: {}: {e}", def.name, view.features[*k].name));
        }
        return Ok(Some(built.mesh));
    }
    let mut values = BTreeMap::new();
    for (k, v) in &view.params {
        let value = circuiter_model::digital::expand(v, &param)
            .map_err(|e| format!("{}: 3D parameter {k}: {e}", def.name))?;
        values.insert(k.clone(), value);
    }
    let mesh = packages::generate(&view.package, &|k| values.get(k).cloned())
        .map_err(|e| format!("{}: {e}", def.name))?;
    Ok(Some(mesh))
}

/// Where a symbol placement puts a body: X along the sheet's X, Y up the sheet (the
/// sheet's Y runs down), turned and mirrored as the symbol is.
pub fn placement(t: &Transform) -> Mat4 {
    let world = |p: Point| -> V3 { [p.x.mm() as f32, -(p.y.mm() as f32), 0.0] };
    let o = world(t.apply(Point::mm(0.0, 0.0)));
    let x = crate::math::sub(world(t.apply(Point::mm(1.0, 0.0))), o);
    // The body's +Y is the symbol's −Y (up on the sheet).
    let y = crate::math::sub(world(t.apply(Point::mm(0.0, -1.0))), o);
    // Up stays up: a mirrored symbol mirrors the body, it doesn't turn it over.
    let z = [0.0, 0.0, 1.0];
    Mat4([
        [x[0], x[1], x[2], 0.0],
        [y[0], y[1], y[2], 0.0],
        [z[0], z[1], z[2], 0.0],
        [o[0], o[1], o[2], 1.0],
    ])
}

/// Every part of a sheet in 3D.
#[derive(Debug, Clone, Default)]
pub struct Assembly {
    /// Each placed body with its instance name.
    pub parts: Vec<(String, Mesh)>,
    /// The board under them.
    pub board: Mesh,
    /// Instances with no 3D body (sources, sub-circuits, logic blocks…).
    pub without: Vec<String>,
    pub errors: Vec<String>,
}

impl Assembly {
    /// The sheet of cell `id`.
    pub fn sheet(design: &Design, id: &CellId) -> Assembly {
        let mut out = Assembly::default();
        let Some(schematic) = design.cell(id).and_then(|d| d.schematic.as_ref()) else {
            return out;
        };
        for inst in &schematic.instances {
            let Some((_, def)) = design.resolve(&inst.cell, &id.library) else { continue };
            match body(def, &inst.params) {
                Ok(Some(mesh)) => out
                    .parts
                    .push((inst.name.clone(), mesh.transformed(&placement(&inst.placement)))),
                Ok(None) => out.without.push(inst.name.clone()),
                Err(e) => out.errors.push(format!("{}: {e}", inst.name)),
            }
        }
        let bounds = out.parts.iter().filter_map(|(_, m)| m.bounds()).reduce(|a, b| {
            (
                [a.0[0].min(b.0[0]), a.0[1].min(b.0[1]), a.0[2].min(b.0[2])],
                [a.1[0].max(b.1[0]), a.1[1].max(b.1[1]), a.1[2].max(b.1[2])],
            )
        });
        if let Some((lo, hi)) = bounds {
            let margin = 6.0;
            out.board = cuboid(
                [lo[0] - margin, lo[1] - margin, -BOARD],
                [hi[0] + margin, hi[1] + margin, 0.0],
                color("board"),
            );
        }
        out
    }

    /// Everything as one mesh, board included.
    pub fn mesh(&self) -> Mesh {
        let mut m = self.board.clone();
        for (_, p) in &self.parts {
            m.append(p.clone());
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::Rotation;

    #[test]
    fn placements_follow_the_symbol() {
        let close = |a: V3, b: V3| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
        let t = Transform::at(Point::mm(10.0, 20.0));
        assert!(close(placement(&t).point([1.0, 2.0, 3.0]), [11.0, -18.0, 3.0]));
        // A quarter turn counter-clockwise on screen turns the body the same way seen
        // from above.
        let t = Transform { rotation: Rotation::R90, ..Transform::at(Point::mm(0.0, 0.0)) };
        assert!(close(placement(&t).point([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // Mirrored left to right.
        let t = Transform { mirror: true, ..Transform::at(Point::mm(0.0, 0.0)) };
        assert!(close(placement(&t).point([1.0, 0.0, 0.0]), [-1.0, 0.0, 0.0]));
        assert!(close(placement(&t).point([0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]), "still upright");
    }
}
