//! Per-capability subtree roll-ups (FR-13): walk `child` links from a task over
//! itself + all descendants. Status → progress % + done/total, TimeSpent →
//! sum, Estimate → total sum + `remaining` (non-`Done` tasks only), Schedule →
//! earliest due, Assignments → union of assignees (spec §13 Q3 default:
//! progress = done/total).

use std::collections::{BTreeMap, BTreeSet};

use todoapp_core::{
    Assignments, ComponentStore, Due, Duration, Id, Schedule, Status, TaskEntityStore,
};

use crate::service::{Error, Services};

#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    pub total: usize,
    pub done: usize,
    pub progress: f32,
    pub time_spent: Duration,
    pub estimate: Duration,
    /// `Estimate` summed over tasks whose `Status` is not `Done` — the TUI's
    /// eta projection input (spec: no partial credit for `TimeSpent`).
    pub remaining: Duration,
    pub earliest_due: Option<Due>,
    pub assignees: BTreeSet<Id>,
    /// Worst-case (lowest-`rank`) `Status` over the task + its descendants —
    /// only `Done` when every task in the subtree is `Done`.
    pub status: Status,
    /// Count of subtree tasks at each `Status`.
    pub by_status: BTreeMap<Status, usize>,
}

impl Default for Aggregate {
    fn default() -> Self {
        Self {
            total: 0,
            done: 0,
            progress: 0.0,
            time_spent: Duration::ZERO,
            estimate: Duration::ZERO,
            remaining: Duration::ZERO,
            earliest_due: None,
            assignees: BTreeSet::new(),
            status: Status::Draft,
            by_status: BTreeMap::new(),
        }
    }
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    pub async fn aggregate(&self, id: &Id) -> Result<Aggregate, Error> {
        let mut agg = Aggregate::default();
        // Roll up over `id` + descendants. Iterative (not recursive `fold`) to
        // avoid boxing an async recursion; the roll-ups are order-independent.
        // Each capability reads only its own component (spec §3 per-cap roll-up).
        let mut ids = self.descendants(id).await;
        ids.insert(id.clone());
        // One bulk read per capability instead of one `get` per id per
        // capability (`aggregate_reads` batches it at the store/adapter).
        let reads = self.store.aggregate_reads(&ids).await;
        let mut worst: Option<Status> = None;
        for tid in &ids {
            agg.total += 1;
            let status = reads.status.get(tid).copied();
            if status == Some(Status::Done) {
                agg.done += 1;
            }
            let status = status.unwrap_or(Status::Draft);
            *agg.by_status.entry(status).or_insert(0) += 1;
            worst = Some(match worst {
                Some(w) if w.rank() <= status.rank() => w,
                _ => status,
            });
            agg.time_spent += reads.time_spent.get(tid).map_or(Duration::ZERO, |t| t.0);
            let estimate = reads.estimate.get(tid).map_or(Duration::ZERO, |e| e.0);
            agg.estimate += estimate;
            if status != Status::Done {
                agg.remaining += estimate;
            }
            if let Some(Schedule(due)) = reads.schedule.get(tid) {
                agg.earliest_due = Some(match agg.earliest_due.take() {
                    Some(cur) if cur <= *due => cur,
                    _ => *due,
                });
            }
            if let Some(Assignments(asg)) = reads.assignments.get(tid) {
                agg.assignees.extend(asg.iter().map(|a| a.actor.clone()));
            }
        }
        agg.progress = if agg.total > 0 {
            agg.done as f32 / agg.total as f32
        } else {
            0.0
        };
        agg.status = worst.unwrap_or(Status::Draft);
        Ok(agg)
    }
}
