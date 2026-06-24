//! In-memory store adapter for tests + fast dev.
//!
//! Capability-keyed storage (spec §3/§7), ECS/column-store style: a generic
//! `comps` map holds each component as a typed `Box<dyn Any>` keyed by
//! `(Component::NAME, id)` — the presence of an entry *is* the capability, and
//! adding a new capability needs **no change here**. `meta` is the minimal `task`
//! entity (timestamps). One `MemStore` backs every port via interior mutability.
//! Deterministic `Clock`/`IdGenerator` fixtures live in `tda-conformance`.
//!
//! ponytail: `RefCell` + `Box<dyn Any>` (single-threaded tests). Swap for `Mutex`
//! if a binary ever shares a `MemStore` across threads.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use async_trait::async_trait;
use tda_core::{
    Collection, CollectionRepository, Component, ComponentStore, Filter, Id, Link, LinkKind,
    LinkRepository, QueryEngine, TaskEntityStore, Timestamp, select_matching,
};

/// Every capability component, type-erased and keyed by `(Component::NAME, id)`.
type Comps = HashMap<(&'static str, Id), Box<dyn Any>>;

/// `meta` = the minimal entity (timestamps); `comps` = every capability component.
#[derive(Default)]
pub struct MemStore {
    meta: RefCell<HashMap<Id, (Timestamp, Timestamp)>>, // (created_at, updated_at)
    comps: RefCell<Comps>,
    links: RefCell<Vec<Link>>,
    collections: RefCell<Vec<Collection>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait(?Send)]
impl ComponentStore for MemStore {
    async fn get<C: Component>(&self, id: &Id) -> Option<C> {
        self.comps
            .borrow()
            .get(&(C::NAME, id.clone()))
            .and_then(|b| b.downcast_ref::<C>().cloned())
    }
    async fn set<C: Component>(&self, id: &Id, value: C) {
        self.comps
            .borrow_mut()
            .insert((C::NAME, id.clone()), Box::new(value));
    }
    async fn remove<C: Component>(&self, id: &Id) {
        self.comps.borrow_mut().remove(&(C::NAME, id.clone()));
    }
}

#[async_trait(?Send)]
impl TaskEntityStore for MemStore {
    async fn create(&self, id: &Id, created: Timestamp, updated: Timestamp) {
        self.meta
            .borrow_mut()
            .insert(id.clone(), (created, updated));
    }
    async fn touch(&self, id: &Id, updated: Timestamp) {
        if let Some(m) = self.meta.borrow_mut().get_mut(id) {
            m.1 = updated;
        }
    }
    async fn meta(&self, id: &Id) -> Option<(Timestamp, Timestamp)> {
        self.meta.borrow().get(id).copied()
    }
    async fn delete(&self, id: &Id) {
        self.meta.borrow_mut().remove(id);
        self.comps.borrow_mut().retain(|(_, k), _| k != id);
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

#[async_trait(?Send)]
impl QueryEngine for MemStore {
    /// No SQL to push to — reuse the core reference scan (spec §7).
    async fn select(&self, filter: &Filter, today: &str) -> Vec<Id> {
        select_matching(self, filter, today).await
    }
}
