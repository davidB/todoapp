//! tda use cases — orchestrate the domain core through the ports (spec §5, §10).
//!
//! Every mutation is a method on [`Services`]: build a command → `decide` →
//! `apply` → persist (task-local), or graph-validated structure ops.

mod aggregate;
mod io;
mod query;
mod service;
mod tasks;

pub use aggregate::Aggregate;
pub use io::Export;
pub use query::QueryHit;
pub use service::{Error, Services, TaskSnapshot};
pub use tasks::Anchor;
