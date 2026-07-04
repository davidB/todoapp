//! The decider pattern (spec §5a): `decide` runs a command through an ordered
//! list of guards, then emits events; `apply` writes those events back as
//! components. Both are **async over a [`ComponentStore`]** (spec §5, superseding
//! `[DECISION]`): a guard reads only the capabilities it needs via `get`, and
//! `apply` touches only what changed via `set`/`remove` — no whole-task aggregate.
//!
//! Scope: the *task-local* lifecycle commands live here. Structural commands
//! (move, link) need the graph and so are validated in `tda-app` — that's where
//! the tree/DAG live. Both still flow through guard-style checks (FR-26).

use crate::model::{
    Assignment, Assignments, Estimate, Id, IssueRef, Notes, Recurrence, Schedule, Status, Tags,
    TimeSpent, Title,
};
use crate::ports::ComponentStore;
use crate::temporal::{Due, Duration};

/// A refused command, with a human/agent-readable reason (spec §5a).
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display, derive_more::Error)]
#[display("{_0}")]
pub struct Denied(#[error(not(source))] pub String);

/// Intent to mutate one task. (`Create` is [`TaskState::new`], not a command.)
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetTitle(String),
    SetNotes(Option<String>),
    SetStatus(Status),
    SetSchedule(Option<Due>),
    SetEstimate(Option<Duration>),
    AddTimeSpent(Duration),
    AddTag(String),
    RemoveTag(String),
    Assign(Id),
    Unassign(Id),
    Claim(Id),
    SetRecurrence(Option<Recurrence>),
    SetIssueRef(Option<IssueRef>),
}

/// The decided result of a command, folded by [`apply`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TitleSet(String),
    NotesSet(Option<String>),
    StatusSet(Status),
    ScheduleSet(Option<Due>),
    EstimateSet(Option<Duration>),
    TimeSpentAdded(Duration),
    TagAdded(String),
    TagRemoved(String),
    Assigned(Id),
    Unassigned(Id),
    /// Sets `wip` and marks (or adds) the claimer's assignment as claimed.
    Claimed(Id),
    RecurrenceSet(Option<Recurrence>),
    IssueRefSet(Option<IssueRef>),
}

/// Facts a guard needs beyond the task itself. Just the derived `blocked` flag
/// for now (spec §8 start-gate); grow as systems need more.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecideCtx {
    pub blocked: bool,
}

/// Run `cmd` through the ordered guards (first denial wins, spec §13 Q2 default),
/// reading current state from `store`. On allow, emit the events. Async: guards
/// `get` only the capabilities they inspect.
pub async fn decide<St: ComponentStore>(
    store: &St,
    id: &Id,
    cmd: &Command,
    ctx: &DecideCtx,
) -> Result<Vec<Event>, Denied> {
    if let Some(d) = g_blocked_start(cmd, ctx) {
        return Err(d);
    }
    if let Some(d) = g_claim_rules(store, id, cmd).await {
        return Err(d);
    }
    Ok(events_for(store, id, cmd).await)
}

/// Write an event back as components (spec §7 presence-as-capability): a
/// collection that becomes empty is `remove`d, not stored empty.
pub async fn apply<St: ComponentStore>(store: &St, id: &Id, event: &Event) {
    match event {
        Event::TitleSet(t) => store.set(id, Title(t.clone())).await,
        Event::NotesSet(Some(n)) => store.set(id, Notes(n.clone())).await,
        Event::NotesSet(None) => store.remove::<Notes>(id).await,
        // A recurring task doesn't stay `done`: it resets in place (spec
        // decision — no per-occurrence spawning). No-op if it has no
        // `Schedule` to advance from.
        Event::StatusSet(Status::Done) => {
            match (
                store.get::<Recurrence>(id).await,
                store.get::<Schedule>(id).await,
            ) {
                (Some(rec), Some(sched)) => {
                    store.set(id, Schedule(rec.next_due(sched.0))).await;
                    store.set(id, Status::Todo).await;
                }
                _ => store.set(id, Status::Done).await,
            }
        }
        Event::StatusSet(s) => store.set(id, *s).await,
        Event::ScheduleSet(Some(d)) => store.set(id, Schedule(*d)).await,
        Event::ScheduleSet(None) => store.remove::<Schedule>(id).await,
        Event::EstimateSet(Some(e)) => store.set(id, Estimate(*e)).await,
        Event::EstimateSet(None) => store.remove::<Estimate>(id).await,
        Event::TimeSpentAdded(m) => {
            let cur = store
                .get::<TimeSpent>(id)
                .await
                .map_or(Duration::ZERO, |t| t.0);
            store.set(id, TimeSpent(cur + *m)).await;
        }
        Event::TagAdded(t) => {
            let mut tags = store.get::<Tags>(id).await.unwrap_or_default();
            tags.0.insert(t.clone());
            store.set(id, tags).await;
        }
        Event::TagRemoved(t) => {
            let mut tags = store.get::<Tags>(id).await.unwrap_or_default();
            tags.0.remove(t);
            detach_if_empty(store, id, tags.0.is_empty(), tags).await;
        }
        Event::Assigned(a) => {
            let mut asg = store.get::<Assignments>(id).await.unwrap_or_default();
            asg.0.push(Assignment {
                actor: a.clone(),
                claimed: false,
            });
            store.set(id, asg).await;
        }
        Event::Unassigned(a) => {
            let mut asg = store.get::<Assignments>(id).await.unwrap_or_default();
            asg.0.retain(|x| &x.actor != a);
            detach_if_empty(store, id, asg.0.is_empty(), asg).await;
        }
        Event::Claimed(a) => {
            store.set(id, Status::Wip).await;
            let mut asg = store.get::<Assignments>(id).await.unwrap_or_default();
            match asg.0.iter_mut().find(|x| &x.actor == a) {
                Some(x) => x.claimed = true,
                None => asg.0.push(Assignment {
                    actor: a.clone(),
                    claimed: true,
                }),
            }
            store.set(id, asg).await;
        }
        Event::RecurrenceSet(Some(r)) => store.set(id, r.clone()).await,
        Event::RecurrenceSet(None) => store.remove::<Recurrence>(id).await,
        Event::IssueRefSet(Some(r)) => store.set(id, r.clone()).await,
        Event::IssueRefSet(None) => store.remove::<IssueRef>(id).await,
    }
}

/// `set` a collection component, or `remove` it when it just became empty.
async fn detach_if_empty<St: ComponentStore, C: crate::model::Component>(
    store: &St,
    id: &Id,
    empty: bool,
    value: C,
) {
    if empty {
        store.remove::<C>(id).await;
    } else {
        store.set(id, value).await;
    }
}

/// Map an allowed command to its events. No-ops (idempotent re-sets) yield `[]`,
/// read from the store's current values.
async fn events_for<St: ComponentStore>(store: &St, id: &Id, cmd: &Command) -> Vec<Event> {
    match cmd {
        Command::SetTitle(t) => {
            let cur = store.get::<Title>(id).await.map(|x| x.0);
            no_op_or(cur.as_ref() == Some(t), Event::TitleSet(t.clone()))
        }
        Command::SetNotes(n) => {
            let cur = store.get::<Notes>(id).await.map(|x| x.0);
            no_op_or(&cur == n, Event::NotesSet(n.clone()))
        }
        Command::SetStatus(s) => {
            let cur = store.get::<Status>(id).await;
            no_op_or(cur.as_ref() == Some(s), Event::StatusSet(*s))
        }
        Command::SetSchedule(d) => {
            let cur = store.get::<Schedule>(id).await.map(|x| x.0);
            no_op_or(&cur == d, Event::ScheduleSet(*d))
        }
        Command::SetEstimate(e) => {
            let cur = store.get::<Estimate>(id).await.map(|x| x.0);
            no_op_or(&cur == e, Event::EstimateSet(*e))
        }
        Command::AddTimeSpent(m) if *m == Duration::ZERO => vec![],
        Command::AddTimeSpent(m) => vec![Event::TimeSpentAdded(*m)],
        Command::AddTag(t) => {
            let has = store.get::<Tags>(id).await.is_some_and(|x| x.0.contains(t));
            no_op_or(has, Event::TagAdded(t.clone()))
        }
        Command::RemoveTag(t) => {
            let has = store.get::<Tags>(id).await.is_some_and(|x| x.0.contains(t));
            no_op_or(!has, Event::TagRemoved(t.clone()))
        }
        Command::Assign(a) => {
            let has = assigned(store, id, a).await;
            no_op_or(has, Event::Assigned(a.clone()))
        }
        Command::Unassign(a) => {
            let has = assigned(store, id, a).await;
            no_op_or(!has, Event::Unassigned(a.clone()))
        }
        Command::Claim(a) => vec![Event::Claimed(a.clone())],
        Command::SetRecurrence(r) => {
            let cur = store.get::<Recurrence>(id).await;
            no_op_or(&cur == r, Event::RecurrenceSet(r.clone()))
        }
        Command::SetIssueRef(r) => {
            let cur = store.get::<IssueRef>(id).await;
            no_op_or(&cur == r, Event::IssueRefSet(r.clone()))
        }
    }
}

/// `[]` when the command is a no-op, else the single resulting event.
fn no_op_or(is_no_op: bool, event: Event) -> Vec<Event> {
    if is_no_op { vec![] } else { vec![event] }
}

async fn assigned<St: ComponentStore>(store: &St, id: &Id, actor: &Id) -> bool {
    store
        .get::<Assignments>(id)
        .await
        .is_some_and(|a| a.0.iter().any(|x| &x.actor == actor))
}

// ---- guards ----------------------------------------------------------------

/// `blocks` system: cannot start (`→wip`, via SetStatus or Claim) while blocked.
fn g_blocked_start(cmd: &Command, ctx: &DecideCtx) -> Option<Denied> {
    let starting = matches!(cmd, Command::SetStatus(Status::Wip) | Command::Claim(_));
    if ctx.blocked && starting {
        return Some(Denied("blocked: a blocker is not done".into()));
    }
    None
}

/// `Assignment` capability: claim only from `todo`; if assignees exist, only a
/// listed one may claim (FR-11, §8).
async fn g_claim_rules<St: ComponentStore>(store: &St, id: &Id, cmd: &Command) -> Option<Denied> {
    if let Command::Claim(actor) = cmd {
        if store.get::<Status>(id).await != Some(Status::Todo) {
            return Some(Denied("claim allowed only from todo".into()));
        }
        let asg = store.get::<Assignments>(id).await.unwrap_or_default();
        if !asg.0.is_empty() && !asg.0.iter().any(|a| &a.actor == actor) {
            return Some(Denied("claim restricted to assignees".into()));
        }
    }
    None
}
