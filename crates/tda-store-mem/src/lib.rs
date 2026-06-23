//! In-memory store adapter for tests + fast dev.
//!
//! Storage is one map per capability keyed by `TaskId` (spec §7: the ECS data
//! shape without an engine) — the presence of an entry *is* the capability.
//! `load` re-assembles a [`TaskState`] for the requested projection; `save`
//! decomposes one back into the maps. One `MemStore` backs all three
//! repository ports via interior mutability. Also provides deterministic
//! [`SeqIds`] and [`FixedClock`] for tests.
//!
//! ponytail: `RefCell` (single-threaded tests). Swap for `Mutex` if a binary
//! ever shares a `MemStore` across threads.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use async_trait::async_trait;
use tda_core::{
    Assignment, Clock, Collection, CollectionRepository, Id, IdGenerator, Link, LinkKind,
    LinkRepository, Projection, Status, TaskRepository, TaskState, Timestamp,
};

/// Component maps keyed by task id (spec §7). The `meta` map is the minimal
/// `task` entity (timestamps); presence in `meta` means the task exists.
#[derive(Default)]
pub struct MemStore {
    meta: RefCell<HashMap<Id, (Timestamp, Timestamp)>>, // (created_at, updated_at)
    titles: RefCell<HashMap<Id, String>>,
    statuses: RefCell<HashMap<Id, Status>>,
    notes: RefCell<HashMap<Id, String>>,
    schedules: RefCell<HashMap<Id, String>>,
    estimates: RefCell<HashMap<Id, u32>>,
    timespent: RefCell<HashMap<Id, u32>>,
    tags: RefCell<HashMap<Id, BTreeSet<String>>>,
    assignments: RefCell<HashMap<Id, Vec<Assignment>>>,
    links: RefCell<Vec<Link>>,
    collections: RefCell<Vec<Collection>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Insert a present optional capability, or detach it when absent.
fn upsert<V: Clone>(map: &RefCell<HashMap<Id, V>>, id: &Id, value: Option<V>) {
    match value {
        Some(v) => {
            map.borrow_mut().insert(id.clone(), v);
        }
        None => {
            map.borrow_mut().remove(id);
        }
    }
}

#[async_trait(?Send)]
impl TaskRepository for MemStore {
    async fn load(&self, id: &Id, projection: Projection) -> Option<TaskState> {
        let (created_at, updated_at) = *self.meta.borrow().get(id)?;
        let mut state = TaskState {
            id: id.clone(),
            // required capabilities (defensive defaults if a row is missing)
            title: self.titles.borrow().get(id).cloned().unwrap_or_default(),
            status: self
                .statuses
                .borrow()
                .get(id)
                .copied()
                .unwrap_or(Status::Draft),
            notes: None,
            due_date: None,
            eta_minutes: None,
            time_spent_minutes: 0,
            tags: BTreeSet::new(),
            assignments: Vec::new(),
            created_at,
            updated_at,
        };
        if matches!(projection, Projection::Full) {
            state.notes = self.notes.borrow().get(id).cloned();
            state.due_date = self.schedules.borrow().get(id).cloned();
            state.eta_minutes = self.estimates.borrow().get(id).copied();
            state.time_spent_minutes = self.timespent.borrow().get(id).copied().unwrap_or(0);
            state.tags = self.tags.borrow().get(id).cloned().unwrap_or_default();
            state.assignments = self
                .assignments
                .borrow()
                .get(id)
                .cloned()
                .unwrap_or_default();
        }
        Some(state)
    }

    async fn save(&self, s: &TaskState) {
        self.meta
            .borrow_mut()
            .insert(s.id.clone(), (s.created_at, s.updated_at));
        self.titles
            .borrow_mut()
            .insert(s.id.clone(), s.title.clone());
        self.statuses.borrow_mut().insert(s.id.clone(), s.status);
        upsert(&self.notes, &s.id, s.notes.clone());
        upsert(&self.schedules, &s.id, s.due_date.clone());
        upsert(&self.estimates, &s.id, s.eta_minutes);
        // 0 / empty ⇒ the capability is absent
        upsert(
            &self.timespent,
            &s.id,
            (s.time_spent_minutes != 0).then_some(s.time_spent_minutes),
        );
        upsert(
            &self.tags,
            &s.id,
            (!s.tags.is_empty()).then(|| s.tags.clone()),
        );
        upsert(
            &self.assignments,
            &s.id,
            (!s.assignments.is_empty()).then(|| s.assignments.clone()),
        );
    }

    async fn delete(&self, id: &Id) {
        self.meta.borrow_mut().remove(id);
        self.titles.borrow_mut().remove(id);
        self.statuses.borrow_mut().remove(id);
        self.notes.borrow_mut().remove(id);
        self.schedules.borrow_mut().remove(id);
        self.estimates.borrow_mut().remove(id);
        self.timespent.borrow_mut().remove(id);
        self.tags.borrow_mut().remove(id);
        self.assignments.borrow_mut().remove(id);
    }

    async fn all(&self) -> Vec<Id> {
        self.meta.borrow().keys().cloned().collect()
    }
}

#[async_trait(?Send)]
impl LinkRepository for MemStore {
    async fn put(&self, link: Link) {
        let mut links = self.links.borrow_mut();
        links.retain(|l| !(l.from == link.from && l.to == link.to && l.kind == link.kind));
        links.push(link);
    }
    async fn remove(&self, from: &Id, to: &Id, kind: LinkKind) {
        self.links
            .borrow_mut()
            .retain(|l| !(&l.from == from && &l.to == to && l.kind == kind));
    }
    async fn outgoing(&self, from: &Id, kind: LinkKind) -> Vec<Link> {
        let mut out: Vec<Link> = self
            .links
            .borrow()
            .iter()
            .filter(|l| &l.from == from && l.kind == kind)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.position.0.total_cmp(&b.position.0));
        out
    }
    async fn incoming(&self, to: &Id, kind: LinkKind) -> Vec<Link> {
        self.links
            .borrow()
            .iter()
            .filter(|l| &l.to == to && l.kind == kind)
            .cloned()
            .collect()
    }
}

#[async_trait(?Send)]
impl CollectionRepository for MemStore {
    async fn save(&self, collection: Collection) {
        let mut cs = self.collections.borrow_mut();
        cs.retain(|c| c.id != collection.id);
        cs.push(collection);
    }
    async fn get(&self, id: &Id) -> Option<Collection> {
        self.collections
            .borrow()
            .iter()
            .find(|c| &c.id == id)
            .cloned()
    }
    async fn by_name(&self, name: &str) -> Option<Collection> {
        self.collections
            .borrow()
            .iter()
            .find(|c| c.name == name)
            .cloned()
    }
    async fn all(&self) -> Vec<Collection> {
        self.collections.borrow().clone()
    }
}

/// Sequential id generator: `t1`, `t2`, … Deterministic for tests/snapshots.
pub struct SeqIds {
    n: RefCell<u64>,
}

impl Default for SeqIds {
    fn default() -> Self {
        Self { n: RefCell::new(0) }
    }
}

impl IdGenerator for SeqIds {
    fn next_id(&self) -> Id {
        let mut n = self.n.borrow_mut();
        *n += 1;
        Id::new(format!("t{n}"))
    }
}

/// Clock pinned to a fixed instant and date — deterministic for tests.
pub struct FixedClock {
    pub now: Timestamp,
    pub today: String,
}

impl Default for FixedClock {
    fn default() -> Self {
        Self {
            now: Timestamp(0),
            today: "2026-06-22".to_string(),
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.now
    }
    fn today(&self) -> String {
        self.today.clone()
    }
}
