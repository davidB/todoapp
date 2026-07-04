//! Entities, capability components, and value objects (spec §3, §7).
//!
//! Storage is one component per capability (spec §7): the durable `task` entity
//! is just identity + timestamps, and each capability is a separate component
//! whose *presence* means the task has it. [`TaskState`] is the in-memory
//! *aggregate* — a task assembled from the components a caller projected (see
//! [`crate::Projection`]) — and is what `decide`/`apply` operate on.

use jiff::ToSpan;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

use crate::temporal::{Date, Due, Duration, Time};

/// Stable identity for tasks, actors, collections. Opaque string (a random ULID
/// in real adapters; a sequence in tests). Serializes transparently as that
/// string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub String);

impl Id {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The invisible structural root (spec §7 virtual-root sentinel). Never a
    /// `task` entity — only ever a `child` link `from`. The reserved string
    /// can't collide with a 26-char base32 ULID.
    pub fn root() -> Self {
        Self("__root__".into())
    }
    pub fn is_root(&self) -> bool {
        self.0 == "__root__"
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Required `Status` capability (spec §8). `blocked` is *derived*, not stored.
/// Transitions between any two values are unrestricted (no guard) — `rank` is
/// just for ordering/display, not a legality check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Draft,
    Todo,
    Wip,
    Paused,
    Done,
}

impl Status {
    /// Position in the `draft→todo→wip→paused→done` chain, for ordering/display only.
    pub fn rank(self) -> i8 {
        match self {
            Status::Draft => 0,
            Status::Todo => 1,
            Status::Wip => 2,
            Status::Paused => 3,
            Status::Done => 4,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Status::Draft => "draft",
            Status::Todo => "todo",
            Status::Wip => "wip",
            Status::Paused => "paused",
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

/// A capability component (spec §3): a unit of data keyed by task `Id` in the
/// store. **Presence of the value *is* the capability** — there is no monolithic
/// `Task` struct; a task is the set of components attached to its id, fetched and
/// mutated one capability at a time (`store.get::<Status>(id)` /
/// `store.set(id, Status::Wip)`). `NAME` keys the per-capability map/table
/// (spec §7). Adding a capability = a new `Component` type; the generic store
/// needs no change.
///
/// The in-memory store only needs `Clone + 'static` (typed `Box<dyn Any>`); the
/// serde bounds are for durable stores that map a component to its row(s).
///
/// The `Serialize`/`DeserializeOwned` bound lets a store map a component
/// generically to/from its row(s): the Turso adapter (M2) bridges each value
/// through `serde_json::to_value`/`from_value` to its typed `c_*` column(s).
pub trait Component: Clone + 'static + Serialize + serde::de::DeserializeOwned {
    const NAME: &'static str;
}

/// Required `Title` capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Title(pub String);
impl Component for Title {
    const NAME: &'static str = "title";
}

/// Required `Status` capability (the enum is the component value itself).
impl Component for Status {
    const NAME: &'static str = "status";
}

/// `Notes` capability: Markdown body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes(pub String);
impl Component for Notes {
    const NAME: &'static str = "notes";
}

/// `Schedule` capability: a due date, optionally with a time-of-day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule(pub Due);
impl Component for Schedule {
    const NAME: &'static str = "schedule";
}

/// `Estimate` capability (effort estimate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate(pub Duration);
impl Component for Estimate {
    const NAME: &'static str = "estimate";
}

/// `TimeSpent` capability (accumulated time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSpent(pub Duration);
impl Component for TimeSpent {
    const NAME: &'static str = "timespent";
}

/// `Tags` capability: the whole set is one component value (empty ⇒ remove it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tags(pub BTreeSet<String>);
impl Component for Tags {
    const NAME: &'static str = "tags";
}

/// `Assignment` capability: the whole assignee list is one component value
/// (empty ⇒ remove it). Its presence/contents drive `Claim` (spec §8).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignments(pub Vec<Assignment>);
impl Component for Assignments {
    const NAME: &'static str = "assignments";
}

/// `IssueRef` capability: a static reference to an external issue tracker's
/// issue (e.g. imported from another tool). `provider` is freeform (no closed
/// enum) — no live sync, no computed URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRef {
    pub provider: String,
    pub id: String,
    pub url: Option<String>,
}
impl Component for IssueRef {
    const NAME: &'static str = "issueref";
}

/// A day of the week, for [`RepeatCycle::Weekly`]. A local enum (not jiff's)
/// so serde stays as simple as [`Status`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    fn from_jiff(w: jiff::civil::Weekday) -> Self {
        match w.to_monday_zero_offset() {
            0 => Weekday::Mon,
            1 => Weekday::Tue,
            2 => Weekday::Wed,
            3 => Weekday::Thu,
            4 => Weekday::Fri,
            5 => Weekday::Sat,
            _ => Weekday::Sun,
        }
    }
}

/// A recurrence rule (spec §3): how often a [`Recurrence`]-carrying task's due
/// date advances when it's completed (see `Recurrence::next_due`). Covers the
/// common cases (daily interval, weekly weekday set, monthly same-day) — not a
/// full RRULE engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatCycle {
    Daily { every_n_days: u32 },
    Weekly { weekdays: BTreeSet<Weekday> },
    Monthly { every_n_months: u32 },
}

/// `Recurrence` capability: a task carrying this **resets in place** on
/// completion instead of staying `done` — spec decision: no per-occurrence
/// task spawning, the same task's `Schedule` advances and its `Status` goes
/// back to `todo` (see the `Event::StatusSet(Status::Done)` apply arm).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    pub cycle: RepeatCycle,
    /// Time-of-day to carry onto the recomputed due date; falls back to the
    /// current due's time if unset.
    pub time: Option<Time>,
}
impl Component for Recurrence {
    const NAME: &'static str = "recurrence";
}

impl Recurrence {
    /// The next due date/time after `current`, per this rule.
    pub fn next_due(&self, current: Due) -> Due {
        let date = match &self.cycle {
            RepeatCycle::Daily { every_n_days } => {
                let n = i64::from((*every_n_days).max(1));
                current
                    .date
                    .0
                    .checked_add(n.days())
                    .map(Date)
                    .unwrap_or(current.date)
            }
            RepeatCycle::Weekly { weekdays } => next_weekday(current.date, weekdays),
            RepeatCycle::Monthly { every_n_months } => {
                let n = i64::from((*every_n_months).max(1));
                current
                    .date
                    .0
                    .checked_add(n.months())
                    .map(Date)
                    .unwrap_or(current.date)
            }
        };
        Due {
            date,
            time: self.time.or(current.time),
        }
    }
}

/// The next date after `from` whose weekday is in `weekdays` (or, if empty,
/// just the next day — an under-specified rule still advances).
fn next_weekday(from: Date, weekdays: &BTreeSet<Weekday>) -> Date {
    let mut d = from.0;
    for _ in 0..7 {
        d = d.checked_add(1.day()).unwrap_or(d);
        if weekdays.is_empty() || weekdays.contains(&Weekday::from_jiff(d.weekday())) {
            return Date(d);
        }
    }
    from
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
    Before(Date),
    On(Date),
    After(Date),
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

#[cfg(test)]
mod recurrence_tests {
    use super::*;

    fn due(s: &str) -> Due {
        Due::parse(s).unwrap()
    }

    #[test]
    fn daily_advances_by_n_days() {
        let rec = Recurrence {
            cycle: RepeatCycle::Daily { every_n_days: 3 },
            time: None,
        };
        assert_eq!(rec.next_due(due("2026-07-01")).date, due("2026-07-04").date);
    }

    #[test]
    fn weekly_finds_next_matching_weekday() {
        // 2026-07-01 is a Wednesday; next Mon/Wed/Fri after it is Friday.
        let rec = Recurrence {
            cycle: RepeatCycle::Weekly {
                weekdays: BTreeSet::from([Weekday::Mon, Weekday::Wed, Weekday::Fri]),
            },
            time: None,
        };
        assert_eq!(rec.next_due(due("2026-07-01")).date, due("2026-07-03").date);
    }

    #[test]
    fn monthly_advances_by_n_months_same_day() {
        let rec = Recurrence {
            cycle: RepeatCycle::Monthly { every_n_months: 1 },
            time: None,
        };
        assert_eq!(rec.next_due(due("2026-07-15")).date, due("2026-08-15").date);
    }

    #[test]
    fn recurrence_time_wins_over_carried_time() {
        let rec = Recurrence {
            cycle: RepeatCycle::Daily { every_n_days: 1 },
            time: Some(Time::parse("09:00").unwrap()),
        };
        let next = rec.next_due(due("2026-07-01 18:00"));
        assert_eq!(next.time, Some(Time::parse("09:00").unwrap()));
    }
}
