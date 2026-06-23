//! Query views (FR-23..FR-25): a filter + sort evaluated over the store, each
//! hit carrying its breadcrumb path (FR-14). Pure given the store snapshot.

use std::cmp::Ordering;

use tda_core::{Dir, DueFilter, Filter, Id, Query, SortField, SortKey, Status, Task};

use crate::service::Services;

/// A task in a query result, with its ancestor titles (root → parent).
#[derive(Debug, Clone, PartialEq)]
pub struct QueryHit {
    pub task: Task,
    pub path: Vec<String>,
}

impl<'a> Services<'a> {
    pub fn evaluate(&self, q: &Query) -> Vec<QueryHit> {
        let today = self.clock.today();
        let within = q.filter.within.as_ref().map(|id| self.descendants(id));

        let mut hits: Vec<QueryHit> = self
            .tasks
            .all()
            .into_iter()
            .filter(|t| self.matches(t, &q.filter, &today, within.as_ref()))
            .map(|t| {
                let path = self.breadcrumb(&t.id);
                QueryHit { task: t, path }
            })
            .collect();

        let keys = if q.sort.is_empty() {
            vec![SortKey {
                key: SortField::Priority,
                dir: Dir::Asc,
            }]
        } else {
            q.sort.clone()
        };
        hits.sort_by(|a, b| self.cmp_hits(a, b, &keys));
        hits
    }

    // ---- built-in parameterized queries (FR-25) ---------------------------

    /// `what-next`: `status:todo` by priority.
    pub fn what_next(&self) -> Vec<QueryHit> {
        self.what_next_for(None, None, None)
    }

    /// `what-next-for`: `status:todo` optionally scoped by assignee/subtree/tag.
    pub fn what_next_for(
        &self,
        assignee: Option<Id>,
        within: Option<Id>,
        tag: Option<String>,
    ) -> Vec<QueryHit> {
        self.evaluate(&Query {
            filter: Filter {
                status: vec![Status::Todo],
                assignee,
                within,
                tags: tag.into_iter().collect(),
                ..Default::default()
            },
            sort: vec![SortKey {
                key: SortField::Priority,
                dir: Dir::Asc,
            }],
        })
    }

    /// `due-today`: `due:today`, sorted by due then priority.
    pub fn due_today(&self) -> Vec<QueryHit> {
        self.evaluate(&Query {
            filter: Filter {
                due: Some(DueFilter::Today),
                ..Default::default()
            },
            sort: vec![
                SortKey {
                    key: SortField::Due,
                    dir: Dir::Asc,
                },
                SortKey {
                    key: SortField::Priority,
                    dir: Dir::Asc,
                },
            ],
        })
    }

    // ---- internals --------------------------------------------------------

    fn matches(
        &self,
        t: &Task,
        f: &Filter,
        today: &str,
        within: Option<&std::collections::HashSet<Id>>,
    ) -> bool {
        if let Some(text) = &f.text {
            let needle = text.to_lowercase();
            let hay = format!("{} {}", t.title, t.notes.as_deref().unwrap_or("")).to_lowercase();
            if !hay.contains(&needle) {
                return false;
            }
        }
        if !f.status.is_empty() && !f.status.contains(&t.status) {
            return false;
        }
        if let Some(a) = &f.assignee
            && !t.assignments.iter().any(|x| &x.actor == a)
        {
            return false;
        }
        if !f.tags.iter().all(|tag| t.tags.contains(tag)) {
            return false;
        }
        if let Some(set) = within
            && !set.contains(&t.id)
        {
            return false;
        }
        if let Some(due) = &f.due
            && !due_matches(t.due_date.as_deref(), due, today)
        {
            return false;
        }
        if let Some(claimed) = f.claimed {
            let any = t.assignments.iter().any(|a| a.claimed);
            if any != claimed {
                return false;
            }
        }
        true
    }

    /// Ancestor titles from root down to the immediate parent.
    fn breadcrumb(&self, id: &Id) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = self.parent_of(id);
        while let Some(pid) = cur {
            if let Some(t) = self.tasks.get(&pid) {
                chain.push(t.title.clone());
                cur = self.parent_of(&pid);
            } else {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// Tree-priority key: the path of `position`s root → task. Sorts a flat
    /// result back into tree order.
    fn priority_key(&self, id: &Id) -> Vec<f64> {
        let mut key = Vec::new();
        let mut cur = id.clone();
        while let Some(parent) = self.parent_of(&cur) {
            let pos = self
                .children_of(&parent)
                .into_iter()
                .find(|l| l.to == cur)
                .map(|l| l.position.0)
                .unwrap_or(0.0);
            key.push(pos);
            cur = parent;
        }
        key.reverse();
        key
    }

    fn cmp_hits(&self, a: &QueryHit, b: &QueryHit, keys: &[SortKey]) -> Ordering {
        for k in keys {
            let ord = match k.key {
                SortField::Priority => cmp_f64_seq(
                    &self.priority_key(&a.task.id),
                    &self.priority_key(&b.task.id),
                ),
                SortField::Due => a.task.due_date.cmp(&b.task.due_date),
                SortField::Created => a.task.created_at.cmp(&b.task.created_at),
                SortField::Updated => a.task.updated_at.cmp(&b.task.updated_at),
            };
            let ord = match k.dir {
                Dir::Asc => ord,
                Dir::Desc => ord.reverse(),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        // stable, deterministic tie-break
        a.task.id.cmp(&b.task.id)
    }
}

fn due_matches(due: Option<&str>, filter: &DueFilter, today: &str) -> bool {
    let Some(d) = due else { return false };
    match filter {
        DueFilter::Today => d == today,
        DueFilter::Overdue => d < today,
        DueFilter::Before(x) => d < x.as_str(),
        DueFilter::On(x) => d == x.as_str(),
        DueFilter::After(x) => d > x.as_str(),
    }
}

/// Lexicographic compare of position paths (f64 has no `Ord`).
fn cmp_f64_seq(a: &[f64], b: &[f64]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        let o = x.total_cmp(y);
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}
