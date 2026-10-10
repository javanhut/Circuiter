//! Building example sheets: parts placed on a grid, each pin on a short wire ending in
//! a net label (same-named labels are connected).

use circuiter_model::Design;
use circuiter_model::cell::{CellDef, CellRef};
use circuiter_model::geom::{Length, Point, Rotation, Transform};
use circuiter_model::schematic::{Instance, NetLabel, Schematic, Wire};
use circuiter_model::symbol::PinDirection;

pub const GRID: f64 = 2.54;

pub struct Sheet<'a> {
    pub design: &'a Design,
    /// The library the sheet is in (for references without one).
    pub library: &'a str,
    pub s: Schematic,
}

#[allow(dead_code)]
impl<'a> Sheet<'a> {
    pub fn new(design: &'a Design, library: &'a str) -> Self {
        Sheet { design, library, s: Schematic::default() }
    }

    pub fn cell(&self, cell: &str) -> (CellRef, &'a CellDef) {
        let r: CellRef = cell.parse().expect("cell reference");
        let (_, def) =
            self.design.resolve(&r, self.library).unwrap_or_else(|| panic!("no cell {cell}"));
        (r, def)
    }

    /// Places `cell` with its top-left corner near (x, y) in grid steps.
    pub fn part(
        &mut self,
        cell: &str,
        name: &str,
        x: i64,
        y: i64,
        params: &[(&str, &str)],
    ) -> usize {
        let (r, def) = self.cell(cell);
        let sym = def.symbol.as_ref().unwrap_or_else(|| panic!("{cell} has no symbol"));
        // Shift so the symbol's top-left pin row starts at (x, y).
        let top = sym.pins.iter().map(|p| p.at.y.nm()).min().unwrap_or(0);
        let left = sym.pins.iter().map(|p| p.at.x.nm()).min().unwrap_or(0);
        let g = Length::from_mm(GRID).nm();
        let at = Point::new(Length(x * g - left), Length(y * g - top));
        self.s.instances.push(Instance {
            fields: Default::default(),
            name: name.into(),
            cell: r,
            placement: Transform { position: at, rotation: Rotation::R0, mirror: false },
            params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            view: None,
        });
        self.s.instances.len() - 1
    }

    /// Where a pin is and which way is out of the part.
    pub fn pin(&self, inst: usize, port: &str) -> (Point, (i64, i64)) {
        let i = &self.s.instances[inst];
        let (_, def) = self.cell(&i.cell.to_string());
        let p = def.symbol.as_ref().unwrap().pins.iter().find(|p| p.port == port);
        let p = p.unwrap_or_else(|| panic!("{} has no pin {port}", i.name));
        let out = match p.direction {
            PinDirection::Left => (1, 0),
            PinDirection::Right => (-1, 0),
            PinDirection::Up => (0, 1),
            PinDirection::Down => (0, -1),
        };
        (i.placement.apply(p.at), out)
    }

    pub fn stub(&mut self, inst: usize, port: &str, steps: i64) -> Point {
        let (at, (dx, dy)) = self.pin(inst, port);
        let g = Length::from_mm(GRID).nm() * steps;
        let end = Point::new(Length(at.x.nm() + dx * g), Length(at.y.nm() + dy * g));
        self.s.wires.push(Wire { points: vec![at, end] });
        end
    }

    /// Connects pins to nets by name: `[("a", "pc"), …]`.
    pub fn nets(&mut self, inst: usize, pairs: &[(&str, &str)]) {
        for (port, net) in pairs {
            let end = self.stub(inst, port, 2);
            self.s.labels.push(NetLabel { global: false, net: net.to_string(), at: end });
        }
    }
}
