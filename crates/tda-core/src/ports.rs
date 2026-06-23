//! Ports: traits the core defines and adapters implement (spec §5).
//!
//! All take `&self` and mutate through interior mutability in the adapter, so a
//! single store can back several repos at once without borrow fights.
//!
//! `QueryEngine` from the spec is intentionally *not* a trait here: query
//! evaluation is pure over a store snapshot and has exactly one implementation
//! (`tda-app`), so a trait would be a one-impl abstraction (YAGNI).

use crate::model::{Collection, Id, Link, LinkKind, Task, Timestamp};

/// Injected time source — deterministic in tests.
pub trait Clock {
    fn now(&self) -> Timestamp;
    /// Today as ISO-8601 `YYYY-MM-DD`, for `due:today` / `overdue`.
    fn today(&self) -> String;
}

/// Injected id source — deterministic in tests.
pub trait IdGenerator {
    fn next_id(&self) -> Id;
}

pub trait TaskRepository {
    fn get(&self, id: &Id) -> Option<Task>;
    /// Insert or replace by id.
    fn put(&self, task: Task);
    fn delete(&self, id: &Id);
    fn all(&self) -> Vec<Task>;
}

pub trait LinkRepository {
    /// Insert or replace the edge keyed by `(from, to, kind)`.
    fn put(&self, link: Link);
    fn remove(&self, from: &Id, to: &Id, kind: LinkKind);
    /// Edges out of `from` of this kind, ordered by `position` ascending.
    fn outgoing(&self, from: &Id, kind: LinkKind) -> Vec<Link>;
    /// Edges into `to` of this kind.
    fn incoming(&self, to: &Id, kind: LinkKind) -> Vec<Link>;
}

pub trait CollectionRepository {
    fn save(&self, collection: Collection);
    fn get(&self, id: &Id) -> Option<Collection>;
    fn by_name(&self, name: &str) -> Option<Collection>;
    fn all(&self) -> Vec<Collection>;
}
