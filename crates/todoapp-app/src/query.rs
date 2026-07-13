//! Query views (FR-23..FR-25): a filter + sort evaluated over the store, each
//! hit carrying its breadcrumb path (FR-14). Pure given the store snapshot.

use std::cmp::Ordering;

use todoapp_core::{
    ComponentStore, Dir, DueFilter, Filter, Id, Query, SortField, SortKey, Status, TaskEntityStore,
    Title,
};

use crate::service::{Services, TaskSnapshot};

/// A task in a query result, with its ancestor titles (root → parent).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueryHit {
    pub task: TaskSnapshot,
    pub path: Vec<String>,
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    pub async fn evaluate(&self, q: &Query) -> Vec<QueryHit> {
        let today = self.clock.today();
        // Filter at the port (Turso → SQL `WHERE`; mem → reference scan); the
        // sort + breadcrumb assembly below is shared and runs the same for any
        // store. `sort_by` is sync, so each hit carries its precomputed
        // tree-priority key for the comparator.
        let mut hits: Vec<(QueryHit, Vec<f64>)> = Vec::new();
        for id in self.query.select(&q.filter, today).await {
            let Ok(t) = self.snapshot(&id).await else {
                continue;
            };
            let path = self.breadcrumb(&t.id).await;
            let key = self.priority_key(&t.id).await;
            hits.push((QueryHit { task: t, path }, key));
        }

        let keys = if q.sort.is_empty() {
            vec![SortKey {
                key: SortField::Priority,
                dir: Dir::Asc,
            }]
        } else {
            q.sort.clone()
        };
        hits.sort_by(|a, b| cmp_hits(a, b, &keys));
        hits.into_iter().map(|(h, _)| h).collect()
    }

    // ---- built-in parameterized queries (FR-25) ---------------------------

    /// `what-next`: `status:todo` by priority.
    pub async fn what_next(&self) -> Vec<QueryHit> {
        self.what_next_for(None, None, None).await
    }

    /// `what-next-for`: `status:todo` optionally scoped by assignee/subtree/tag.
    pub async fn what_next_for(
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
                archived: Some(false),
                ..Default::default()
            },
            sort: vec![SortKey {
                key: SortField::Priority,
                dir: Dir::Asc,
            }],
        })
        .await
    }

    /// `claimable-for`: `what-next` restricted to tasks `actor` may actually
    /// claim (FR-11): unassigned (anyone may claim) or listing `actor`, and not
    /// blocked (the claim guard would deny those anyway).
    pub async fn claimable_for(
        &self,
        actor: &Id,
        within: Option<Id>,
        tag: Option<String>,
    ) -> Vec<QueryHit> {
        // ponytail: post-filter over the todo set, not SQL — N is small.
        let mut out = Vec::new();
        for h in self.what_next_for(None, within, tag).await {
            let allowed = h.task.assignments.is_empty()
                || h.task.assignments.iter().any(|a| &a.actor == actor);
            if allowed && !self.is_blocked(&h.task.id).await {
                out.push(h);
            }
        }
        out
    }

    /// `due-today`: `due:today`, sorted by due then priority.
    pub async fn due_today(&self) -> Vec<QueryHit> {
        self.evaluate(&Query {
            filter: Filter {
                due: Some(DueFilter::Today),
                archived: Some(false),
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
        .await
    }

    // ---- internals --------------------------------------------------------

    /// Ancestor titles from root down to the immediate parent (FR-14).
    pub async fn breadcrumb(&self, id: &Id) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = self.parent_of(id).await;
        while let Some(pid) = cur {
            if let Some(t) = self.store.get::<Title>(&pid).await {
                chain.push(t.0);
                cur = self.parent_of(&pid).await;
            } else {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// Tree-priority key: the path of `position`s root → task. Sorts a flat
    /// result back into tree order.
    async fn priority_key(&self, id: &Id) -> Vec<f64> {
        let mut key = Vec::new();
        let mut cur = id.clone();
        while let Some(parent) = self.parent_of(&cur).await {
            let pos = self
                .children_of(&parent)
                .await
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
}

/// Compare two decorated hits `(hit, precomputed tree-priority key)`.
fn cmp_hits(a: &(QueryHit, Vec<f64>), b: &(QueryHit, Vec<f64>), keys: &[SortKey]) -> Ordering {
    for k in keys {
        let ord = match k.key {
            SortField::Priority => cmp_f64_seq(&a.1, &b.1),
            SortField::Due => a.0.task.due_date.cmp(&b.0.task.due_date),
            SortField::Created => a.0.task.created_at.cmp(&b.0.task.created_at),
            SortField::Updated => a.0.task.updated_at.cmp(&b.0.task.updated_at),
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
    a.0.task.id.cmp(&b.0.task.id)
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
