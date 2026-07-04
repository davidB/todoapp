//! Reference query *filter* over the ports (spec §7 query model): the predicate
//! half of `QueryEngine::select`. A store with no faster path delegates here
//! (the in-memory store); the Turso adapter overrides it with SQL.
//!
//! Pure over `ComponentStore + TaskEntityStore + LinkRepository`, so it is the
//! single source of truth for filter semantics — and directly unit-testable.

use std::collections::HashSet;

use crate::model::{
    Archived, Assignments, DueFilter, Filter, Id, LinkKind, Notes, Schedule, Status, Tags, Title,
};
use crate::ports::{ComponentStore, LinkRepository, TaskEntityStore};
use crate::temporal::Date;

/// Task ids matching `filter` (unsorted). Loads only the components a given
/// predicate needs, matching the in-app snapshot semantics exactly.
pub async fn select_matching<St>(store: &St, filter: &Filter, today: Date) -> Vec<Id>
where
    St: ComponentStore + TaskEntityStore + LinkRepository,
{
    let within = match &filter.within {
        Some(root) => Some(descendants(store, root).await),
        None => None,
    };
    let mut out = Vec::new();
    for id in store.all().await {
        if matches(store, filter, today, within.as_ref(), &id).await {
            out.push(id);
        }
    }
    out
}

async fn matches<St>(
    store: &St,
    f: &Filter,
    today: Date,
    within: Option<&HashSet<Id>>,
    id: &Id,
) -> bool
where
    St: ComponentStore + TaskEntityStore + LinkRepository,
{
    if let Some(set) = within
        && !set.contains(id)
    {
        return false;
    }
    if let Some(text) = &f.text {
        let needle = text.to_lowercase();
        let title = store
            .get::<Title>(id)
            .await
            .map(|t| t.0)
            .unwrap_or_default();
        let notes = store
            .get::<Notes>(id)
            .await
            .map(|n| n.0)
            .unwrap_or_default();
        if !format!("{title} {notes}").to_lowercase().contains(&needle) {
            return false;
        }
    }
    if !f.status.is_empty() {
        let st = store.get::<Status>(id).await.unwrap_or(Status::Draft);
        if !f.status.contains(&st) {
            return false;
        }
    }
    // assignee / claimed both read the assignment list — load once if needed.
    if f.assignee.is_some() || f.claimed.is_some() {
        let asg = store.get::<Assignments>(id).await.unwrap_or_default().0;
        if let Some(a) = &f.assignee
            && !asg.iter().any(|x| &x.actor == a)
        {
            return false;
        }
        if let Some(claimed) = f.claimed
            && asg.iter().any(|x| x.claimed) != claimed
        {
            return false;
        }
    }
    if !f.tags.is_empty() {
        let tags = store.get::<Tags>(id).await.unwrap_or_default().0;
        if !f.tags.iter().all(|t| tags.contains(t)) {
            return false;
        }
    }
    if let Some(due) = &f.due {
        // Overdue/due-today etc. stay day-granularity (spec: a rendez-vous
        // time is display-only, never compared) — only `.date` is read here.
        let date = store.get::<Schedule>(id).await.map(|s| s.0.date);
        if !due_matches(date, due, today) {
            return false;
        }
    }
    if let Some(archived) = f.archived {
        let is_archived = store.get::<Archived>(id).await.is_some();
        if is_archived != archived {
            return false;
        }
    }
    true
}

fn due_matches(due: Option<Date>, filter: &DueFilter, today: Date) -> bool {
    let Some(d) = due else { return false };
    match filter {
        DueFilter::Today => d == today,
        DueFilter::Overdue => d < today,
        DueFilter::Before(x) => d < *x,
        DueFilter::On(x) => d == *x,
        DueFilter::After(x) => d > *x,
    }
}

/// All descendants of `id` via `child` links (excludes `id`). Iterative.
pub async fn descendants<St: LinkRepository + ?Sized>(store: &St, id: &Id) -> HashSet<Id> {
    let mut seen = HashSet::new();
    let mut stack: Vec<Id> = store
        .outgoing(id, LinkKind::Child)
        .await
        .into_iter()
        .map(|l| l.to)
        .collect();
    while let Some(cur) = stack.pop() {
        if seen.insert(cur.clone()) {
            stack.extend(
                store
                    .outgoing(&cur, LinkKind::Child)
                    .await
                    .into_iter()
                    .map(|l| l.to),
            );
        }
    }
    seen
}
