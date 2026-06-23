//! In-memory store adapter (plain composed structs) for tests + fast dev.
//!
//! One `MemStore` backs all three repository ports via interior mutability.
//! Also provides deterministic [`SeqIds`] and [`FixedClock`] for tests.
//!
//! ponytail: `RefCell` (single-threaded tests). Swap for `Mutex` if a binary
//! ever shares a `MemStore` across threads.

use std::cell::RefCell;
use std::collections::HashMap;

use async_trait::async_trait;
use tda_core::{
    Clock, Collection, CollectionRepository, Id, IdGenerator, Link, LinkKind, LinkRepository, Task,
    TaskRepository, Timestamp,
};

#[derive(Default)]
pub struct MemStore {
    tasks: RefCell<HashMap<Id, Task>>,
    links: RefCell<Vec<Link>>,
    collections: RefCell<Vec<Collection>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait(?Send)]
impl TaskRepository for MemStore {
    async fn get(&self, id: &Id) -> Option<Task> {
        self.tasks.borrow().get(id).cloned()
    }
    async fn put(&self, task: Task) {
        self.tasks.borrow_mut().insert(task.id.clone(), task);
    }
    async fn delete(&self, id: &Id) {
        self.tasks.borrow_mut().remove(id);
    }
    async fn all(&self) -> Vec<Task> {
        self.tasks.borrow().values().cloned().collect()
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
