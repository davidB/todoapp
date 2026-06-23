//! The decider pattern (spec §5a): `decide` runs a command through an ordered
//! list of pure guards, then emits events; `apply` folds events into state.
//!
//! Scope: the *task-local* lifecycle commands live here, where a single `TaskState`
//! plus a tiny [`DecideCtx`] is enough to decide. Structural commands (move,
//! link) need the graph and so are validated in `tda-app` — that's where the
//! tree/DAG live. Both still flow through guard-style checks (FR-26).

use crate::model::{Assignment, Id, Status, TaskState};

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
    SetSchedule(Option<String>),
    SetEstimate(Option<u32>),
    AddTimeSpent(u32),
    AddTag(String),
    RemoveTag(String),
    Assign(Id),
    Unassign(Id),
    Claim(Id),
}

/// The decided result of a command, folded by [`apply`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TitleSet(String),
    NotesSet(Option<String>),
    StatusSet(Status),
    ScheduleSet(Option<String>),
    EstimateSet(Option<u32>),
    TimeSpentAdded(u32),
    TagAdded(String),
    TagRemoved(String),
    Assigned(Id),
    Unassigned(Id),
    /// Sets `wip` and marks (or adds) the claimer's assignment as claimed.
    Claimed(Id),
}

/// Facts a guard needs beyond the task itself. Just the derived `blocked` flag
/// for now (spec §8 start-gate); grow as systems need more.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecideCtx {
    pub blocked: bool,
}

type Guard = fn(&TaskState, &Command, &DecideCtx) -> Option<Denied>;

/// Ordered guards; first denial wins (spec §13 Q2 default).
const GUARDS: &[Guard] = &[g_status_transition, g_blocked_start, g_claim_rules];

pub fn decide(task: &TaskState, cmd: &Command, ctx: &DecideCtx) -> Result<Vec<Event>, Denied> {
    for guard in GUARDS {
        if let Some(denied) = guard(task, cmd, ctx) {
            return Err(denied);
        }
    }
    Ok(events_for(task, cmd))
}

pub fn apply(task: &mut TaskState, event: &Event) {
    match event {
        Event::TitleSet(t) => task.title = t.clone(),
        Event::NotesSet(n) => task.notes = n.clone(),
        Event::StatusSet(s) => task.status = *s,
        Event::ScheduleSet(d) => task.due_date = d.clone(),
        Event::EstimateSet(e) => task.eta_minutes = *e,
        Event::TimeSpentAdded(m) => task.time_spent_minutes += m,
        Event::TagAdded(t) => {
            task.tags.insert(t.clone());
        }
        Event::TagRemoved(t) => {
            task.tags.remove(t);
        }
        Event::Assigned(a) => task.assignments.push(Assignment {
            actor: a.clone(),
            claimed: false,
        }),
        Event::Unassigned(a) => task.assignments.retain(|x| &x.actor != a),
        Event::Claimed(a) => {
            task.status = Status::Wip;
            match task.assignments.iter_mut().find(|x| &x.actor == a) {
                Some(x) => x.claimed = true,
                None => task.assignments.push(Assignment {
                    actor: a.clone(),
                    claimed: true,
                }),
            }
        }
    }
}

/// Map an allowed command to its events. No-ops (idempotent re-sets) yield `[]`.
fn events_for(task: &TaskState, cmd: &Command) -> Vec<Event> {
    match cmd {
        Command::SetTitle(t) if &task.title == t => vec![],
        Command::SetTitle(t) => vec![Event::TitleSet(t.clone())],
        Command::SetNotes(n) if &task.notes == n => vec![],
        Command::SetNotes(n) => vec![Event::NotesSet(n.clone())],
        Command::SetStatus(s) if &task.status == s => vec![],
        Command::SetStatus(s) => vec![Event::StatusSet(*s)],
        Command::SetSchedule(d) if &task.due_date == d => vec![],
        Command::SetSchedule(d) => vec![Event::ScheduleSet(d.clone())],
        Command::SetEstimate(e) if &task.eta_minutes == e => vec![],
        Command::SetEstimate(e) => vec![Event::EstimateSet(*e)],
        Command::AddTimeSpent(0) => vec![],
        Command::AddTimeSpent(m) => vec![Event::TimeSpentAdded(*m)],
        Command::AddTag(t) if task.tags.contains(t) => vec![],
        Command::AddTag(t) => vec![Event::TagAdded(t.clone())],
        Command::RemoveTag(t) if task.tags.contains(t) => vec![Event::TagRemoved(t.clone())],
        Command::RemoveTag(_) => vec![],
        Command::Assign(a) if task.assignments.iter().any(|x| &x.actor == a) => vec![],
        Command::Assign(a) => vec![Event::Assigned(a.clone())],
        Command::Unassign(a) if task.assignments.iter().any(|x| &x.actor == a) => {
            vec![Event::Unassigned(a.clone())]
        }
        Command::Unassign(_) => vec![],
        Command::Claim(a) => vec![Event::Claimed(a.clone())],
    }
}

// ---- guards ----------------------------------------------------------------

/// `Status` capability: only single steps along `draft↔todo↔wip↔done` (a re-set
/// to the same value is a no-op, allowed here and dropped by `events_for`).
fn g_status_transition(task: &TaskState, cmd: &Command, _: &DecideCtx) -> Option<Denied> {
    if let Command::SetStatus(to) = cmd
        && (task.status.rank() - to.rank()).abs() > 1
    {
        return Some(Denied(format!(
            "illegal status transition {} -> {}",
            task.status, to
        )));
    }
    None
}

/// `blocks` system: cannot start (`→wip`, via SetStatus or Claim) while blocked.
fn g_blocked_start(_: &TaskState, cmd: &Command, ctx: &DecideCtx) -> Option<Denied> {
    let starting = matches!(cmd, Command::SetStatus(Status::Wip) | Command::Claim(_));
    if ctx.blocked && starting {
        return Some(Denied("blocked: a blocker is not done".into()));
    }
    None
}

/// `Assignment` capability: claim only from `todo`; if assignees exist, only a
/// listed one may claim (FR-11, §8).
fn g_claim_rules(task: &TaskState, cmd: &Command, _: &DecideCtx) -> Option<Denied> {
    if let Command::Claim(actor) = cmd {
        if task.status != Status::Todo {
            return Some(Denied("claim allowed only from todo".into()));
        }
        if !task.assignments.is_empty() && !task.assignments.iter().any(|a| &a.actor == actor) {
            return Some(Denied("claim restricted to assignees".into()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Timestamp;

    fn task(status: Status) -> TaskState {
        TaskState::new(Id::new("t1"), "x", status, Timestamp(0))
    }

    #[test]
    fn status_steps_one_at_a_time() {
        let t = task(Status::Draft);
        assert!(decide(&t, &Command::SetStatus(Status::Todo), &DecideCtx::default()).is_ok());
        // skipping a step is denied
        assert!(decide(&t, &Command::SetStatus(Status::Wip), &DecideCtx::default()).is_err());
    }

    #[test]
    fn cannot_claim_a_draft() {
        let t = task(Status::Draft);
        let r = decide(&t, &Command::Claim(Id::new("a")), &DecideCtx::default());
        assert_eq!(
            r.unwrap_err(),
            Denied("claim allowed only from todo".into())
        );
    }

    #[test]
    fn claim_restricted_to_assignees() {
        let mut t = task(Status::Todo);
        t.assignments.push(Assignment {
            actor: Id::new("alice"),
            claimed: false,
        });
        // bob is not an assignee
        assert!(decide(&t, &Command::Claim(Id::new("bob")), &DecideCtx::default()).is_err());
        // alice may, and claiming sets wip + claimed
        let ev = decide(&t, &Command::Claim(Id::new("alice")), &DecideCtx::default()).unwrap();
        for e in &ev {
            apply(&mut t, e);
        }
        assert_eq!(t.status, Status::Wip);
        assert!(t.assignments[0].claimed);
    }

    #[test]
    fn cannot_start_while_blocked() {
        let t = task(Status::Todo);
        let ctx = DecideCtx { blocked: true };
        assert!(decide(&t, &Command::SetStatus(Status::Wip), &ctx).is_err());
        assert!(decide(&t, &Command::Claim(Id::new("a")), &ctx).is_err());
    }

    #[test]
    fn claim_with_no_assignees_adds_claimer() {
        let mut t = task(Status::Todo);
        let ev = decide(&t, &Command::Claim(Id::new("solo")), &DecideCtx::default()).unwrap();
        for e in &ev {
            apply(&mut t, e);
        }
        assert_eq!(t.assignments.len(), 1);
        assert!(t.assignments[0].claimed);
    }
}
