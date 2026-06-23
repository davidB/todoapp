//! Ports: traits the core defines and adapters implement (spec §5).
//!
//! All take `&self` and mutate through interior mutability in the adapter, so a
//! single store can back several repos at once without borrow fights.
//!
//! **Async boundary (spec §5).** The durable store (Turso, M2) is async, so the
//! repository ports are `async` traits; `Clock`/`IdGenerator` stay sync. The
//! `decide`/`apply` core and query evaluation remain pure & sync.
//!
//! `#[async_trait(?Send)]` keeps the ports dyn-compatible (used behind `&dyn`)
//! without forcing `Send` futures — the in-memory adapter uses `RefCell`.
//! ponytail: revisit to `Send` (Mutex-backed store) only if a multi-threaded
//! server (M5/axum) needs it.
//!
//! `QueryEngine` from the spec is intentionally *not* a trait here: query
//! evaluation is pure over a store snapshot and has exactly one implementation
//! (`tda-app`), so a trait would be a one-impl abstraction (YAGNI).

use async_trait::async_trait;

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

#[async_trait(?Send)]
pub trait TaskRepository {
    async fn get(&self, id: &Id) -> Option<Task>;
    /// Insert or replace by id.
    async fn put(&self, task: Task);
    async fn delete(&self, id: &Id);
    async fn all(&self) -> Vec<Task>;
}

#[async_trait(?Send)]
pub trait LinkRepository {
    /// Insert or replace the edge keyed by `(from, to, kind)`.
    async fn put(&self, link: Link);
    async fn remove(&self, from: &Id, to: &Id, kind: LinkKind);
    /// Edges out of `from` of this kind, ordered by `position` ascending.
    async fn outgoing(&self, from: &Id, kind: LinkKind) -> Vec<Link>;
    /// Edges into `to` of this kind.
    async fn incoming(&self, to: &Id, kind: LinkKind) -> Vec<Link>;
}

#[async_trait(?Send)]
pub trait CollectionRepository {
    async fn save(&self, collection: Collection);
    async fn get(&self, id: &Id) -> Option<Collection>;
    async fn by_name(&self, name: &str) -> Option<Collection>;
    async fn all(&self) -> Vec<Collection>;
}
