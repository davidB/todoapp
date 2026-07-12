//! tda domain core — entities, capability components, the decider, and ports.
//!
//! Zero I/O dependencies (spec §5): only `serde` (serialization) and `derive_more`.

mod command;
mod mentions;
mod model;
mod ports;
mod query;
mod short_id;
mod temporal;
pub mod testing;

pub use command::*;
pub use mentions::*;
pub use model::*;
pub use ports::*;
pub use query::*;
pub use short_id::*;
pub use temporal::*;
