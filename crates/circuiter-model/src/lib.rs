//! Circuiter's unified design model.
//!
//! Everything is built from [`cell::CellDef`]s: components with a pin interface and
//! several views (symbol, schematic, SPICE, ...). Cells live in [`library::Library`]s;
//! a [`project::Project`] owns one library and refers to others; a [`design::Design`]
//! ties the libraries together, resolves references and elaborates the hierarchy.

pub mod annotate;
pub mod board;
pub mod builder;
pub mod cell;
pub mod check;
pub mod connectivity;
pub mod design;
pub mod digital;
pub mod error;
mod format;
pub mod geom;
pub mod library;
pub mod model3d;
pub mod project;
pub mod schematic;
pub mod spice;
pub mod symbol;
pub mod units;

pub use cell::{
    Cell, CellDef, CellRef, ParamDef, ParamKind, Port, PortKind, ViewKind, validate_folder,
};
pub use design::{CellId, Design, HierNode, STD_LIBRARY};
pub use error::{ModelError, Result};
pub use format::{from_ron, to_ron};
pub use geom::{Length, Point, Rotation, Transform};
pub use library::Library;
pub use project::Project;
