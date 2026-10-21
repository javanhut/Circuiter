//! Diagrams drawn automatically: a sheet's parts placed in columns so signals flow left
//! to right, wired as an engineer would wire them, the way RTL viewers show a design;
//! and the RTL view of a Verilog module drawn the same way.

pub mod layout;
pub mod rtl;
pub mod sheet;

pub use sheet::{arrange, arrange_sheet};
