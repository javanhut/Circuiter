//! Cells are Circuiter's unit of reuse: a component with a fixed pin interface and any
//! number of views (symbol, schematic, SPICE model, and later digital/HDL, footprint, 3D).
//!
//! A MOSFET primitive, an inverter built from two MOSFETs, a D flip-flop built from
//! inverters, and a whole CPU are all cells. The interface defined here (ports and
//! parameters) is the contract every view must honour.

use crate::schematic::Schematic;
use crate::spice::SpiceView;
use crate::symbol::Symbol;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The interface and metadata of a cell, stored in `cell.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Reference designator prefix for instances: `R`, `C`, `Q`, `M`, `U`, `X`, ...
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default)]
    pub ports: Vec<Port>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDef>,
    /// Power and ground symbols: every instance's single port joins this global net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_net: Option<String>,
}

fn default_prefix() -> String {
    "X".to_owned()
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            description: String::new(),
            prefix: default_prefix(),
            ports: Vec::new(),
            params: Vec::new(),
            global_net: None,
        }
    }
}

impl Cell {
    pub fn port(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }

    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_passive")]
    pub kind: PortKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

fn is_passive(kind: &PortKind) -> bool {
    *kind == PortKind::Passive
}

/// Electrical type of a port, used by electrical rule checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PortKind {
    Input,
    Output,
    Bidirectional,
    /// Passive two-terminal style connection (resistor leads, transistor terminals).
    #[default]
    Passive,
    /// Supply input that must be driven (VDD, VCC, VEE).
    PowerIn,
    /// Supply source (regulator outputs, sources, global power symbols).
    PowerOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDef {
    pub name: String,
    pub kind: ParamKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamKind {
    /// A physical quantity in engineering notation, e.g. `4.7k` with unit `Ω`.
    Real {
        unit: String,
    },
    Integer,
    Bool,
    Text,
    Choice(Vec<String>),
}

impl ParamKind {
    /// Checks that `value` is acceptable for this parameter kind.
    pub fn validate(&self, value: &str) -> Result<(), String> {
        match self {
            ParamKind::Real { unit } => {
                let unit = (!unit.is_empty()).then_some(unit.as_str());
                crate::units::parse_quantity(value, unit).map(drop).map_err(|e| e.to_string())
            }
            ParamKind::Integer => value
                .trim()
                .parse::<i64>()
                .map(drop)
                .map_err(|_| format!("`{value}` is not an integer")),
            ParamKind::Bool => match value.trim() {
                "true" | "false" => Ok(()),
                _ => Err(format!("`{value}` is not `true` or `false`")),
            },
            ParamKind::Text => Ok(()),
            ParamKind::Choice(options) => {
                if options.iter().any(|o| o == value) {
                    Ok(())
                } else {
                    Err(format!("`{value}` is not one of: {}", options.join(", ")))
                }
            }
        }
    }
}

/// The views a cell can provide. The order of variants is not significant; view
/// precedence is chosen by the design's view order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ViewKind {
    Schematic,
    Spice,
}

impl fmt::Display for ViewKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ViewKind::Schematic => "schematic",
            ViewKind::Spice => "spice",
        })
    }
}

/// A cell together with all of its loaded views.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellDef {
    pub name: String,
    pub cell: Cell,
    pub symbol: Option<Symbol>,
    pub schematic: Option<Schematic>,
    pub spice: Option<SpiceView>,
}

impl CellDef {
    pub fn has_view(&self, view: ViewKind) -> bool {
        match view {
            ViewKind::Schematic => self.schematic.is_some(),
            ViewKind::Spice => self.spice.is_some(),
        }
    }
}

/// A reference to a cell, written `library:cell`, or just `cell` to refer to a cell in
/// the same library as the referencing cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CellRef {
    pub library: Option<String>,
    pub cell: String,
}

impl CellRef {
    pub fn new(library: Option<&str>, cell: &str) -> Self {
        CellRef { library: library.map(str::to_owned), cell: cell.to_owned() }
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.library {
            Some(lib) => write!(f, "{lib}:{}", self.cell),
            None => f.write_str(&self.cell),
        }
    }
}

impl FromStr for CellRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (library, cell) = match s.split_once(':') {
            Some((lib, cell)) => (Some(lib), cell),
            None => (None, s),
        };
        if let Some(lib) = library {
            validate_name(lib).map_err(|e| format!("library name in `{s}`: {e}"))?;
        }
        validate_name(cell).map_err(|e| format!("cell name in `{s}`: {e}"))?;
        Ok(CellRef::new(library, cell))
    }
}

impl TryFrom<String> for CellRef {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl From<CellRef> for String {
    fn from(r: CellRef) -> String {
        r.to_string()
    }
}

/// Validates a library, cell, port, parameter or instance name.
///
/// Names become directory names and SPICE identifiers, so they are restricted to ASCII
/// letters, digits and underscores, starting with a letter.
pub fn validate_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    match chars.next() {
        None => return Err("name is empty".to_owned()),
        Some(c) if !c.is_ascii_alphabetic() => {
            return Err(format!("`{name}` must start with a letter"));
        }
        _ => {}
    }
    if let Some(bad) = chars.find(|c| !(c.is_ascii_alphanumeric() || *c == '_')) {
        return Err(format!("`{name}` contains `{bad}`; use letters, digits and `_`"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_ref_parses_qualified_and_local() {
        assert_eq!("std:nmos".parse::<CellRef>().unwrap(), CellRef::new(Some("std"), "nmos"));
        assert_eq!("inverter".parse::<CellRef>().unwrap(), CellRef::new(None, "inverter"));
        assert!("std:".parse::<CellRef>().is_err());
        assert!("1bad".parse::<CellRef>().is_err());
        assert!("a:b:c".parse::<CellRef>().is_err());
    }

    #[test]
    fn param_kinds_validate_values() {
        let real = ParamKind::Real { unit: "Ω".to_owned() };
        assert!(real.validate("4k7").is_ok());
        assert!(real.validate("lots").is_err());
        assert!(ParamKind::Integer.validate("3").is_ok());
        assert!(ParamKind::Integer.validate("3.5").is_err());
        assert!(ParamKind::Choice(vec!["a".into(), "b".into()]).validate("c").is_err());
    }
}
