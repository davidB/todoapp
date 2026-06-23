//! Entities, capability components, and value objects (spec §3, §7).
//!
//! Composition is by *sparse fields*: a `Task` carries the optional capabilities
//! it has (`notes`, `due_date`, …). This mirrors the §7 storage mapping (nullable
//! columns / side tables) and is the plain-struct composition the spec calls for —
//! no trait-object capability registry (YAGNI until a second store needs it).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Stable identity for tasks, actors, collections. Opaque string (UUIDv7 in real
/// adapters; a sequence in tests). Serializes transparently as that string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub String);

impl Id {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Unix-epoch milliseconds, supplied by the [`crate::Clock`] port. Sortable;
/// avoids pulling a date library into the core. Serializes as a bare integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(pub i64);

/// Required `Status` capability (spec §8). `blocked` is *derived*, not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Draft,
    Todo,
    Wip,
    Done,
}

impl Status {
    /// Position in the `draft→todo→wip→done` chain; adjacency drives legal steps.
    pub fn rank(self) -> i8 {
        match self {
            Status::Draft => 0,
            Status::Todo => 1,
            Status::Wip => 2,
            Status::Done => 3,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Status::Draft => "draft",
            Status::Todo => "todo",
            Status::Wip => "wip",
            Status::Done => "done",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Person,
    Agent,
}

/// A human or agent. Not persisted via a port in M1 (the spec lists no
/// `ActorRepository`); `Assignment`/`Claim` only ever reference an actor `Id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: Id,
    pub kind: ActorKind,
    pub name: String,
}

/// One assignee on a task; `claimed` flips when that actor claims it (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub actor: Id,
    pub claimed: bool,
}

/// The atomic unit. Identity + required capabilities (`title`, `status`) + the
/// optional capabilities it happens to carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub title: String,
    pub status: Status,
    pub notes: Option<String>,
    /// `Schedule` capability: ISO-8601 date `YYYY-MM-DD`. Lexical order == date
    /// order, so `due:today`/`overdue` are plain string compares (no date crate).
    pub due_date: Option<String>,
    /// `Estimate` capability.
    pub eta_minutes: Option<u32>,
    /// `TimeSpent` capability.
    pub time_spent_minutes: u32,
    pub tags: BTreeSet<String>,
    pub assignments: Vec<Assignment>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    /// Construct a fresh task (the `Create` command is a constructor, not a
    /// guarded mutation — nothing to deny but an empty title).
    pub fn new(id: Id, title: impl Into<String>, status: Status, at: Timestamp) -> Self {
        Task {
            id,
            title: title.into(),
            status,
            notes: None,
            due_date: None,
            eta_minutes: None,
            time_spent_minutes: 0,
            tags: BTreeSet::new(),
            assignments: Vec::new(),
            created_at: at,
            updated_at: at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Child,
    Blocks,
}

/// Fractional index (spec §7): insert between two neighbours by averaging, so a
/// reorder or subtree move touches one row, never the siblings.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Position(pub f64);

impl Position {
    /// A position strictly between `before` and `after` (either may be open).
    pub fn between(before: Option<f64>, after: Option<f64>) -> f64 {
        match (before, after) {
            (None, None) => 0.0,
            (Some(b), None) => b + 1.0,
            (None, Some(a)) => a - 1.0,
            (Some(b), Some(a)) => (b + a) / 2.0,
        }
    }
}

/// A typed, ordered directed edge. `child` is a single-parent tree; `blocks` is
/// a DAG (invariants enforced in `tda-app`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub from: Id,
    pub to: Id,
    pub kind: LinkKind,
    pub position: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind {
    Tree,
    Query,
}

/// A saved tree or saved query (spec §7). `spec` holds the query for `query` kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: Id,
    pub name: String,
    pub kind: CollectionKind,
    pub spec: Option<Query>,
}

// ---- Query model (spec §7 "Query model") ----------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    #[serde(default)]
    pub filter: Filter,
    #[serde(default)]
    pub sort: Vec<SortKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub text: Option<String>,
    #[serde(default)]
    pub status: Vec<Status>,
    pub assignee: Option<Id>,
    /// all-of (spec §13 default).
    #[serde(default)]
    pub tags: Vec<String>,
    pub within: Option<Id>,
    pub due: Option<DueFilter>,
    pub claimed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DueFilter {
    Today,
    Overdue,
    Before(String),
    On(String),
    After(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortField {
    Priority,
    Due,
    Created,
    Updated,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SortKey {
    pub key: SortField,
    pub dir: Dir,
}
