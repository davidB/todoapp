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

use crate::model::{Collection, Id, Link, LinkKind, TaskState, Timestamp};

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

/// Which capability components to assemble when loading a task (spec §5
/// load-by-projection: a caller names the set it needs, so heavy components
/// like `Notes` are read only when wanted).
///
/// ponytail: two projections cover M1 (tree rows want title+status; mutations
/// and query/aggregate want everything). Widen to an explicit capability set
/// only when a third view needs a different slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// Identity + required capabilities (`title`, `status`) only — tree/list rows.
    Row,
    /// Identity + every capability — detail panes and any mutation.
    Full,
}

#[async_trait(?Send)]
pub trait TaskRepository {
    /// Assemble the task to `projection`, or `None` if it has no identity.
    async fn load(&self, id: &Id, projection: Projection) -> Option<TaskState>;
    /// Upsert (create or update): write identity + the capability components
    /// present in `state`, detaching any that are absent. The minimal entity
    /// (id + timestamps) and presence-as-capability live here (spec §7).
    async fn save(&self, state: &TaskState);
    async fn delete(&self, id: &Id);
    /// Ids of every stored task; callers `load` the projection they need.
    async fn all(&self) -> Vec<Id>;
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
