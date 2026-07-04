//! `Services`: the bundle of ports the use cases run against, plus shared graph
//! helpers and the `decide → apply → persist` mutation runner.
//!
//! `Services` is generic over the concrete component store `St` (spec §5
//! capability-keyed access: `ComponentStore`'s generic `get::<C>` is not
//! object-safe, so it can't be a `&dyn`). The graph/collection/clock/id ports
//! stay `&dyn` — they have no generic methods.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use tda_core::{
    Assignment, Assignments, Clock, CollectionRepository, Command, ComponentStore, DecideCtx,
    Denied, Due, Duration, Estimate, Id, IdGenerator, LinkKind, LinkRepository, Notes, QueryEngine,
    Recurrence, Schedule, Status, Tags, TaskEntityStore, TimeSpent, Timestamp, Title, apply,
    decide,
};

pub struct Services<'a, St> {
    pub store: &'a St,
    pub links: &'a dyn LinkRepository,
    pub collections: &'a dyn CollectionRepository,
    pub query: &'a dyn QueryEngine,
    pub clock: &'a dyn Clock,
    pub ids: &'a dyn IdGenerator,
}

/// A read-only view of a task assembled from its components — for query results,
/// export, and as the return of a mutation. **Never** fed back to `decide`/`apply`
/// (those work capability-keyed via the store); assembling here is one-way, so
/// there is no partial-aggregate to reconcile. Decomposed back to components by
/// [`Services::write_snapshot`] on import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: Id,
    pub title: String,
    pub status: Status,
    pub notes: Option<String>,
    pub due_date: Option<Due>,
    pub eta_minutes: Option<Duration>,
    pub time_spent_minutes: Duration,
    pub tags: BTreeSet<String>,
    pub assignments: Vec<Assignment>,
    pub recurrence: Option<Recurrence>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From, PartialEq)]
pub enum Error {
    #[from(skip)]
    #[display("task not found: {_0}")]
    NotFound(#[error(not(source))] Id),
    #[display("denied: {_0}")]
    Denied(Denied),
    #[from(skip)]
    #[display("would create a cycle: {_0}")]
    Cycle(#[error(not(source))] String),
    #[from(skip)]
    #[display("import error: {_0}")]
    Import(#[error(not(source))] String),
    #[from(skip)]
    #[display("ambiguous id: {_0}")]
    AmbiguousId(#[error(not(source))] String),
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    /// Resolve a user-typed id or short prefix (git/jj-style abbreviation,
    /// spec-independent — see [`tda_core::resolve_id_prefix`]) against every
    /// id currently in the store. The CLI/TUI entry point for letting a human
    /// type a short id instead of the full ULID.
    pub async fn resolve_id(&self, typed: &str) -> Result<Id, Error> {
        let ids = self.store.all().await;
        match tda_core::resolve_id_prefix(&ids, typed) {
            tda_core::ResolvedId::Found(id) => Ok(id),
            tda_core::ResolvedId::NotFound => Err(Error::NotFound(Id::new(typed))),
            tda_core::ResolvedId::Ambiguous(matches) => {
                let candidates = matches
                    .iter()
                    .map(Id::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(Error::AmbiguousId(format!(
                    "{typed:?} matches {candidates}"
                )))
            }
        }
    }

    /// Assemble a read-only [`TaskSnapshot`] from the task's components.
    pub async fn snapshot(&self, id: &Id) -> Result<TaskSnapshot, Error> {
        let (created_at, updated_at) = self
            .store
            .meta(id)
            .await
            .ok_or_else(|| Error::NotFound(id.clone()))?;
        Ok(TaskSnapshot {
            id: id.clone(),
            title: self
                .store
                .get::<Title>(id)
                .await
                .map(|t| t.0)
                .unwrap_or_default(),
            status: self.store.get::<Status>(id).await.unwrap_or(Status::Draft),
            notes: self.store.get::<Notes>(id).await.map(|n| n.0),
            due_date: self.store.get::<Schedule>(id).await.map(|s| s.0),
            eta_minutes: self.store.get::<Estimate>(id).await.map(|e| e.0),
            time_spent_minutes: self
                .store
                .get::<TimeSpent>(id)
                .await
                .map_or(Duration::ZERO, |t| t.0),
            tags: self
                .store
                .get::<Tags>(id)
                .await
                .map(|t| t.0)
                .unwrap_or_default(),
            assignments: self
                .store
                .get::<Assignments>(id)
                .await
                .map(|a| a.0)
                .unwrap_or_default(),
            recurrence: self.store.get::<Recurrence>(id).await,
            created_at,
            updated_at,
        })
    }

    /// Decompose a snapshot back into the entity + its present components
    /// (presence-as-capability, spec §7). Used by import.
    pub(crate) async fn write_snapshot(&self, t: &TaskSnapshot) {
        self.store.create(&t.id, t.created_at, t.updated_at).await;
        self.store.set(&t.id, Title(t.title.clone())).await;
        self.store.set(&t.id, t.status).await;
        if let Some(n) = &t.notes {
            self.store.set(&t.id, Notes(n.clone())).await;
        }
        if let Some(d) = t.due_date {
            self.store.set(&t.id, Schedule(d)).await;
        }
        if let Some(e) = t.eta_minutes {
            self.store.set(&t.id, Estimate(e)).await;
        }
        if t.time_spent_minutes != Duration::ZERO {
            self.store.set(&t.id, TimeSpent(t.time_spent_minutes)).await;
        }
        if !t.tags.is_empty() {
            self.store.set(&t.id, Tags(t.tags.clone())).await;
        }
        if !t.assignments.is_empty() {
            self.store
                .set(&t.id, Assignments(t.assignments.clone()))
                .await;
        }
        if let Some(r) = &t.recurrence {
            self.store.set(&t.id, r.clone()).await;
        }
    }

    /// Child links out of `parent`, ordered by position.
    pub async fn children_of(&self, parent: &Id) -> Vec<tda_core::Link> {
        self.links.outgoing(parent, LinkKind::Child).await
    }

    /// The structural parent of `child`, if any (single-parent tree). The
    /// virtual-root sentinel maps to `None` — a root has no *visible* parent.
    pub async fn parent_of(&self, child: &Id) -> Option<Id> {
        self.links
            .incoming(child, LinkKind::Child)
            .await
            .into_iter()
            .next()
            .map(|l| l.from)
            .filter(|p| !p.is_root())
    }

    /// The raw structural parent, **including** the virtual-root sentinel — for
    /// move/reorder, which must re-point or order the actual `child` edge a root
    /// holds to the sentinel (unlike the public, sentinel-hiding `parent_of`).
    pub(crate) async fn raw_parent_of(&self, child: &Id) -> Option<Id> {
        self.links
            .incoming(child, LinkKind::Child)
            .await
            .into_iter()
            .next()
            .map(|l| l.from)
    }

    /// Top-level tasks: the invisible root's children, ordered by position
    /// (spec §7). Port-level via `outgoing(ROOT, Child)` — indexed in Turso,
    /// no whole-store scan.
    pub async fn roots(&self) -> Vec<Id> {
        self.children_of(&Id::root())
            .await
            .into_iter()
            .map(|l| l.to)
            .collect()
    }

    /// Derived `blocked` (spec §8): some incoming `blocks` edge whose blocker
    /// task is not `done`.
    pub async fn is_blocked(&self, id: &Id) -> bool {
        for l in self.links.incoming(id, LinkKind::Blocks).await {
            if self
                .store
                .get::<Status>(&l.from)
                .await
                .is_some_and(|s| s != Status::Done)
            {
                return true;
            }
        }
        false
    }

    /// All descendants of `id` via `child` links (excludes `id`).
    pub async fn descendants(&self, id: &Id) -> HashSet<Id> {
        tda_core::descendants(self.links, id).await
    }

    /// Run a task-local command through `decide → apply → persist` (spec §5a),
    /// capability-keyed: `decide`/`apply` read and write components via the store.
    pub async fn run(&self, id: &Id, cmd: Command) -> Result<TaskSnapshot, Error> {
        if self.store.meta(id).await.is_none() {
            return Err(Error::NotFound(id.clone()));
        }
        let ctx = DecideCtx {
            blocked: self.is_blocked(id).await,
        };
        let events = decide(self.store, id, &cmd, &ctx).await?;
        for e in &events {
            apply(self.store, id, e).await;
        }
        if !events.is_empty() {
            self.store.touch(id, self.clock.now()).await;
        }
        self.snapshot(id).await
    }
}
