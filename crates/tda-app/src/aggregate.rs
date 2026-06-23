//! Per-capability subtree roll-ups (FR-13): walk `child` links from a task over
//! itself + all descendants. Status → progress %, TimeSpent → sum, Estimate →
//! sum, Schedule → earliest due (spec §13 Q3 default: progress = done/total).

use tda_core::{ComponentStore, Estimate, Id, Schedule, Status, TaskEntityStore, TimeSpent};

use crate::service::{Error, Services};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Aggregate {
    pub total: usize,
    pub done: usize,
    pub progress: f32,
    pub time_spent_minutes: u32,
    pub eta_minutes: u32,
    pub earliest_due: Option<String>,
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    pub async fn aggregate(&self, id: &Id) -> Result<Aggregate, Error> {
        let mut agg = Aggregate::default();
        // Roll up over `id` + descendants. Iterative (not recursive `fold`) to
        // avoid boxing an async recursion; the roll-ups are order-independent.
        // Each capability reads only its own component (spec §3 per-cap roll-up).
        let mut ids = self.descendants(id).await;
        ids.insert(id.clone());
        for tid in ids {
            agg.total += 1;
            if self.store.get::<Status>(&tid).await == Some(Status::Done) {
                agg.done += 1;
            }
            agg.time_spent_minutes += self.store.get::<TimeSpent>(&tid).await.map_or(0, |t| t.0);
            agg.eta_minutes += self.store.get::<Estimate>(&tid).await.map_or(0, |e| e.0);
            if let Some(Schedule(due)) = self.store.get::<Schedule>(&tid).await {
                agg.earliest_due = Some(match agg.earliest_due.take() {
                    Some(cur) if cur <= due => cur,
                    _ => due,
                });
            }
        }
        agg.progress = if agg.total > 0 {
            agg.done as f32 / agg.total as f32
        } else {
            0.0
        };
        Ok(agg)
    }
}
