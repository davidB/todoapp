//! tda domain core — entities, capability components, the decider, and ports.
//!
//! Zero I/O dependencies (spec §5): only `serde` (serialization) and `derive_more`.

mod command;
mod model;
mod ports;

pub use command::*;
pub use model::*;
pub use ports::*;
