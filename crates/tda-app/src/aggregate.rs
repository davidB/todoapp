//! Per-capability subtree roll-ups (FR-13): walk `child` links from a task over
//! itself + all descendants. Status → progress %, TimeSpent → sum, Estimate →
//! sum, Schedule → earliest due (spec §13 Q3 default: progress = done/total).

use tda_core::{Id, Status};

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

impl<'a> Services<'a> {
    pub fn aggregate(&self, id: &Id) -> Result<Aggregate, Error> {
        let mut agg = Aggregate::default();
        self.fold(id, &mut agg)?;
        agg.progress = if agg.total > 0 {
            agg.done as f32 / agg.total as f32
        } else {
            0.0
        };
        Ok(agg)
    }

    fn fold(&self, id: &Id, agg: &mut Aggregate) -> Result<(), Error> {
        let task = self.load(id)?;
        agg.total += 1;
        if task.status == Status::Done {
            agg.done += 1;
        }
        agg.time_spent_minutes += task.time_spent_minutes;
        agg.eta_minutes += task.eta_minutes.unwrap_or(0);
        if let Some(due) = task.due_date {
            agg.earliest_due = Some(match agg.earliest_due.take() {
                Some(cur) if cur <= due => cur,
                _ => due,
            });
        }
        for link in self.children_of(id) {
            self.fold(&link.to, agg)?;
        }
        Ok(())
    }
}
