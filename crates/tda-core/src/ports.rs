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
//! `QueryEngine` is a port (spec §5): the *filter* runs at the store so a
//! durable adapter pushes it into SQL (efficient `WHERE`) instead of an O(n)
//! scan; sort + breadcrumb assembly stays in `tda-app`, shared by every store.
//! [`crate::select_matching`] is the reference scan adapters reuse when they
//! have no faster path (the in-memory store does).

use async_trait::async_trait;

use crate::model::{Collection, Component, Filter, Id, Link, LinkKind, Timestamp};

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

/// Capability-keyed component access (spec §3/§7), ECS/column-store style: read,
/// write, or detach **one capability at a time**, keyed by task `Id`. There is no
/// whole-task load/save — a caller (or a guard) touches only the capabilities it
/// needs. Generic methods make this *not* object-safe, so callers hold a concrete
/// store (`Services<St>`), not `&dyn`.
///
/// ponytail: per-capability reads/writes inside a command mean a command is not
/// one snapshot-in / one-save-out. Single-user/embedded is fine; wrap a command's
/// reads+writes in a transaction at M2 (Turso) if concurrency demands it.
#[async_trait(?Send)]
pub trait ComponentStore {
    /// The task's `C` component, or `None` if it doesn't carry that capability.
    async fn get<C: Component>(&self, id: &Id) -> Option<C>;
    /// Attach/overwrite the task's `C` component.
    async fn set<C: Component>(&self, id: &Id, value: C);
    /// Detach the task's `C` component (absent ⇒ no-op).
    async fn remove<C: Component>(&self, id: &Id);
}

/// The minimal `task` entity (spec §7): identity + timestamps only. Everything
/// else is a [`Component`]. `delete` cascades every component of the id.
#[async_trait(?Send)]
pub trait TaskEntityStore {
    async fn create(&self, id: &Id, created: Timestamp, updated: Timestamp);
    /// Bump `updated_at` (after a mutation produced events).
    async fn touch(&self, id: &Id, updated: Timestamp);
    /// `(created_at, updated_at)`, or `None` if the id has no entity.
    async fn meta(&self, id: &Id) -> Option<(Timestamp, Timestamp)>;
    async fn delete(&self, id: &Id);
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

/// Evaluate a query's *filter* — the ids of tasks that match (unsorted). `today`
/// is the reference date for `due:today`/`overdue`. Object-safe (no generic
/// methods), so it rides in `Services` as `&dyn`. Sorting + breadcrumbs are the
/// caller's job (`tda-app`), identical across stores.
#[async_trait(?Send)]
pub trait QueryEngine {
    async fn select(&self, filter: &Filter, today: &str) -> Vec<Id>;
}
