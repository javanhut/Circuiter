//! Circuiter's analog circuit simulator, in pure Rust.
//!
//! It reads SPICE decks (the ones the netlister writes, and vendor model files) and
//! runs operating-point, DC sweep and transient analyses with SPICE's algorithms:
//! modified nodal analysis, Newton-Raphson with junction voltage limiting, gmin and
//! source stepping for hard operating points, and trapezoidal integration with
//! truncation-error step control. Results are checked against ngspice in the tests.
//!
//! ```
//! use circuiter_sim::{Circuit, Options, op};
//! let c = Circuit::parse("V1 in 0 DC 10\nR1 in out 1k\nR2 out 0 1k").unwrap();
//! let r = op(&c, &Options::default()).unwrap();
//! assert!((r.voltage(&c, "out").unwrap() - 5.0).abs() < 1e-6);
//! ```

mod analysis;
mod circuit;
pub mod device;
pub mod number;
pub mod parse;
mod solve;
pub mod sparse;

pub use analysis::{
    AcResult, Complex, Control, DeviceOp, Inputs, NoObserver, Observer, OpResult, Options,
    SimError, Spacing, Step, TranResult, Transient, ac, dc_sweep, op, tran, tran_observed,
};
pub use circuit::{Circuit, Device, Element, Problem};
