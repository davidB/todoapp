//! Entities, capability components, and value objects (spec §3, §7).
//!
//! Storage is one component per capability (spec §7): the durable `task` entity
//! is just identity + timestamps, and each capability is a separate component
//! whose *presence* means the task has it. [`TaskState`] is the in-memory
//! *aggregate* — a task assembled from the components a caller projected (see
//! [`crate::Projection`]) — and is what `decide`/`apply` operate on.

use jiff::ToSpan;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// True if `self` is `ancestor` or a `/`-delimited descendant of it, e.g.
    /// actor `harness/model` is `is_or_under("harness")`. Lets a specific actor
    /// claim / see tasks assigned to a broader identity (spec §2 hierarchical
    /// assignee): assign the harness, let it pick the model.
    pub fn is_or_under(&self, ancestor: &Id) -> bool {
        self == ancestor
            || self
                .0
                .strip_prefix(&ancestor.0)
                .is_some_and(|rest| rest.starts_with('/'))
    }
    /// Content-addressed id for a blob: same bytes ⇒ same id (cheap incidental
    /// dedup, not a content-hash identity guarantee — collisions are possible
    /// but not a practical concern at this scale). Shared by every `BlobStore`
    /// adapter so they agree on ids for the same bytes.
    pub fn for_blob(bytes: &[u8]) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        Self(format!("blob_{:016x}", h.finish()))
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

/// `TimeLog` capability: a per-day breakdown of time spent, keyed by calendar
/// date. `TimeSpent` stays the fast-path cumulative total — it's recomputed
/// as this map's sum whenever it's set (see the `Event::TimeLogSet` apply
/// arm), so aggregation (FR-13) keeps reading `TimeSpent` unchanged. Mixing
/// this with the plain `AddTimeSpent` command (no date) can leave the two
/// slightly inconsistent — a known, accepted edge case, not guarded against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeLog(pub BTreeMap<Date, Duration>);
impl Component for TimeLog {
    const NAME: &'static str = "timelog";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Link,
    File,
    Image,
}

/// One attachment: a `Link` never has a `blob` (it's just a URL); `File`/
/// `Image` may or may not have one — `url` keeps the original source
/// path/URL either way (e.g. from an import), `blob` is `Some` once actual
/// bytes have been stored via [`crate::BlobStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Id,
    pub kind: AttachmentKind,
    pub title: String,
    pub url: Option<String>,
    pub blob: Option<Id>,
    pub mime: Option<String>,
}

/// `Attachments` capability: the whole list is one component value (empty ⇒
/// remove it), like `Tags`/`Assignments`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachments(pub Vec<Attachment>);
impl Component for Attachments {
    const NAME: &'static str = "attachments";
}

/// `Archived` capability: an orthogonal flag, independent of `Status` (a task
/// can be `done` and archived, or archived without being `done`) — presence
/// *is* the flag, no payload needed. Hidden from default views by callers
/// passing `Filter { archived: Some(false), .. }` (spec §13 Q4 direction);
/// `QueryEngine`/`Filter` itself stay neutral (`None` = no restriction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archived;
impl Component for Archived {
    const NAME: &'static str = "archived";
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

/// `Workspace` capability: binds a task (and, by ancestor lookup, its subtree)
/// to a project folder/repo. `name` is the stable cross-machine identity;
/// `path` is only the *default* local folder — per-machine overrides live in
/// local config (keyed by `name`), never in the store, so a shared database
/// stays portable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub path: Option<String>,
}
impl Component for Workspace {
    const NAME: &'static str = "workspace";
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

    /// A weekday name, case-insensitive: `mon`..`sun` or `monday`..`sunday`.
    /// Shared by `[...]` title syntax and any other free-text due-date input
    /// (TUI edit form, CLI `--due`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "mon" | "monday" => Weekday::Mon,
            "tue" | "tuesday" => Weekday::Tue,
            "wed" | "wednesday" => Weekday::Wed,
            "thu" | "thursday" => Weekday::Thu,
            "fri" | "friday" => Weekday::Fri,
            "sat" | "saturday" => Weekday::Sat,
            "sun" | "sunday" => Weekday::Sun,
            _ => return None,
        })
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
    /// A minimal todoist/org-mode-style recurrence expression, mapped onto
    /// [`RepeatCycle`]'s daily-interval/weekly-weekday-set/monthly-interval
    /// cases — not a full RRULE engine:
    /// `daily` | `every day` | `every N days`
    /// `weekly` | `every week` | `every <weekday>[,<weekday>...]`
    /// `monthly` | `every month` | `every N months`
    ///
    /// Shared by `[...]` title syntax, the CLI `--recurrence` flag, and any
    /// other free-text recurrence input.
    pub fn parse(s: &str) -> Result<Self, String> {
        let lower = s.trim().to_ascii_lowercase();
        let rest = lower.strip_prefix("every").map(str::trim).unwrap_or(&lower);
        let invalid = || format!("unrecognized recurrence expression {s:?}");
        let cycle = match rest {
            "day" if lower.starts_with("every") => RepeatCycle::Daily { every_n_days: 1 },
            "daily" => RepeatCycle::Daily { every_n_days: 1 },
            "week" if lower.starts_with("every") => RepeatCycle::Weekly {
                weekdays: BTreeSet::new(),
            },
            "weekly" => RepeatCycle::Weekly {
                weekdays: BTreeSet::new(),
            },
            "month" if lower.starts_with("every") => RepeatCycle::Monthly { every_n_months: 1 },
            "monthly" => RepeatCycle::Monthly { every_n_months: 1 },
            _ if lower.starts_with("every") => {
                if let Some(n_days) = rest.strip_suffix("days").map(str::trim_end) {
                    RepeatCycle::Daily {
                        every_n_days: n_days.parse().map_err(|_| invalid())?,
                    }
                } else if let Some(n_months) = rest.strip_suffix("months").map(str::trim_end) {
                    RepeatCycle::Monthly {
                        every_n_months: n_months.parse().map_err(|_| invalid())?,
                    }
                } else {
                    let weekdays: Option<BTreeSet<Weekday>> =
                        rest.split(',').map(|w| Weekday::parse(w.trim())).collect();
                    RepeatCycle::Weekly {
                        weekdays: weekdays.ok_or_else(invalid)?,
                    }
                }
            }
            _ => return Err(invalid()),
        };
        Ok(Recurrence { cycle, time: None })
    }

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

/// An unresolved due-date value parsed from `[...]` title syntax (`FR-34`):
/// pure and reference-date-agnostic, since `todoapp-core` has no `Clock`
/// access (spec §5). [`DueSpec::resolve`] turns it into a concrete [`Due`]
/// once a caller (`todoapp-app`, which holds a `Clock`) supplies "today".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueSpec {
    Absolute(Due),
    TimeOnly(Time),
    /// Next occurrence of this weekday, strictly after `today`.
    Weekday(Weekday),
}

impl DueSpec {
    /// Accepts anything [`Due::parse`] does (`YYYY-MM-DD[ HH:MM]`), a bare
    /// `HH:MM` time, or a weekday name — the same relaxed grammar `[...]`
    /// title syntax uses, shared with any other free-text due-date input
    /// (TUI edit form, CLI `--due`).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Ok(due) = Due::parse(s) {
            return Ok(DueSpec::Absolute(due));
        }
        if let Ok(time) = Time::parse(s) {
            return Ok(DueSpec::TimeOnly(time));
        }
        Weekday::parse(s).map(DueSpec::Weekday).ok_or_else(|| {
            format!(
                "expected a date (YYYY-MM-DD[ HH:MM]), a time (HH:MM), or a weekday name, got {s:?}"
            )
        })
    }

    pub fn resolve(&self, today: Date) -> Due {
        match self {
            DueSpec::Absolute(due) => *due,
            DueSpec::TimeOnly(time) => Due {
                date: today,
                time: Some(*time),
            },
            DueSpec::Weekday(weekday) => Due {
                date: next_weekday(today, &BTreeSet::from([*weekday])),
                time: None,
            },
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
/// a DAG (invariants enforced in `todoapp-app`).
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
    /// `None` = no restriction (matches archived and non-archived alike);
    /// hiding archived tasks by default is a caller-side choice, not a
    /// query-engine special case.
    pub archived: Option<bool>,
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
mod id_tests {
    use super::*;

    #[test]
    fn is_or_under_hierarchy() {
        let actor = Id::new("harness/model");
        assert!(actor.is_or_under(&Id::new("harness"))); // broader assignee
        assert!(actor.is_or_under(&Id::new("harness/model"))); // exact
        assert!(!actor.is_or_under(&Id::new("harness/other")));
        assert!(!actor.is_or_under(&Id::new("harnes"))); // prefix, not a segment
        assert!(!Id::new("harness").is_or_under(&Id::new("harness/model"))); // not upward
    }
}

#[cfg(test)]
mod recurrence_tests {
    use rstest::rstest;

    use super::*;

    fn due(s: &str) -> Due {
        Due::parse(s).unwrap()
    }

    #[rstest]
    #[case("daily", RepeatCycle::Daily { every_n_days: 1 })]
    #[case("every day", RepeatCycle::Daily { every_n_days: 1 })]
    #[case("every 3 days", RepeatCycle::Daily { every_n_days: 3 })]
    #[case("weekly", RepeatCycle::Weekly { weekdays: BTreeSet::new() })]
    #[case("every week", RepeatCycle::Weekly { weekdays: BTreeSet::new() })]
    #[case(
        "every mon,wed,fri",
        RepeatCycle::Weekly { weekdays: BTreeSet::from([Weekday::Mon, Weekday::Wed, Weekday::Fri]) }
    )]
    #[case("monthly", RepeatCycle::Monthly { every_n_months: 1 })]
    #[case("every month", RepeatCycle::Monthly { every_n_months: 1 })]
    #[case("every 2 months", RepeatCycle::Monthly { every_n_months: 2 })]
    fn parse_accepts_every_grammar_form(#[case] input: &str, #[case] expected_cycle: RepeatCycle) {
        assert_eq!(
            Recurrence::parse(input),
            Ok(Recurrence {
                cycle: expected_cycle,
                time: None
            })
        );
    }

    #[rstest]
    #[case("every")]
    #[case("every fortnight")]
    #[case("every mon,someday")]
    #[case("hourly")]
    fn parse_rejects_invalid_expressions(#[case] input: &str) {
        assert!(Recurrence::parse(input).is_err());
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

#[cfg(test)]
mod due_spec_tests {
    use rstest::rstest;

    use super::*;

    fn date(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn absolute_passes_through_unchanged() {
        let due = Due::parse("2026-08-01 09:00").unwrap();
        assert_eq!(DueSpec::Absolute(due).resolve(date("2026-07-01")), due);
    }

    #[test]
    fn time_only_resolves_against_today() {
        let time = Time::parse("14:30").unwrap();
        let resolved = DueSpec::TimeOnly(time).resolve(date("2026-07-01"));
        assert_eq!(resolved.date, date("2026-07-01"));
        assert_eq!(resolved.time, Some(time));
    }

    // 2026-07-01 is a Wednesday.
    #[rstest]
    #[case("2026-07-01", Weekday::Wed, "2026-07-08")] // today is Wed -> next Wed, a week out
    #[case("2026-07-01", Weekday::Fri, "2026-07-03")] // later this week
    #[case("2026-07-01", Weekday::Mon, "2026-07-06")] // earlier in the week -> wraps
    fn weekday_resolves_to_next_occurrence_strictly_after_today(
        #[case] today: &str,
        #[case] weekday: Weekday,
        #[case] expected: &str,
    ) {
        let resolved = DueSpec::Weekday(weekday).resolve(date(today));
        assert_eq!(resolved.date, date(expected));
        assert_eq!(resolved.time, None);
    }

    #[rstest]
    #[case("2026-07-20", DueSpec::Absolute(Due::parse("2026-07-20").unwrap()))]
    #[case(
        "2026-07-20 09:00",
        DueSpec::Absolute(Due::parse("2026-07-20 09:00").unwrap())
    )]
    #[case("09:00", DueSpec::TimeOnly(Time::parse("09:00").unwrap()))]
    #[case("fri", DueSpec::Weekday(Weekday::Fri))]
    #[case("Friday", DueSpec::Weekday(Weekday::Fri))]
    fn parse_accepts_every_grammar_form(#[case] input: &str, #[case] expected: DueSpec) {
        assert_eq!(DueSpec::parse(input), Ok(expected));
    }

    #[test]
    fn parse_rejects_nonsense() {
        assert!(DueSpec::parse("not a date").is_err());
    }
}
