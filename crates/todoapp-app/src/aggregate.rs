//! Per-capability subtree roll-ups (FR-13): walk `child` links from a task over
//! itself + all descendants. Status → progress % + done/total, TimeSpent →
//! sum, Estimate → total sum + `remaining` (non-`Done` tasks only), Schedule →
//! earliest due, Assignments → union of assignees (spec §13 Q3 default:
//! progress = done/total).

use std::collections::BTreeSet;

use todoapp_core::{
    Assignments, ComponentStore, Due, Duration, Estimate, Id, Schedule, Status, TaskEntityStore,
    TimeSpent,
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
        let mut worst: Option<Status> = None;
        for tid in ids {
            agg.total += 1;
            let status = self.store.get::<Status>(&tid).await;
            if status == Some(Status::Done) {
                agg.done += 1;
            }
            let status = status.unwrap_or(Status::Draft);
            worst = Some(match worst {
                Some(w) if w.rank() <= status.rank() => w,
                _ => status,
            });
            agg.time_spent += self
                .store
                .get::<TimeSpent>(&tid)
                .await
                .map_or(Duration::ZERO, |t| t.0);
            let estimate = self
                .store
                .get::<Estimate>(&tid)
                .await
                .map_or(Duration::ZERO, |e| e.0);
            agg.estimate += estimate;
            if status != Status::Done {
                agg.remaining += estimate;
            }
            if let Some(Schedule(due)) = self.store.get::<Schedule>(&tid).await {
                agg.earliest_due = Some(match agg.earliest_due.take() {
                    Some(cur) if cur <= due => cur,
                    _ => due,
                });
            }
            if let Some(Assignments(asg)) = self.store.get::<Assignments>(&tid).await {
                agg.assignees.extend(asg.into_iter().map(|a| a.actor));
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
