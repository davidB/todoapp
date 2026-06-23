//! `Services`: the bundle of ports the use cases run against, plus shared graph
//! helpers and the `decide → apply → persist` mutation runner.

use std::collections::HashSet;

use tda_core::{
    Clock, CollectionRepository, Command, DecideCtx, Denied, Id, IdGenerator, LinkKind,
    LinkRepository, Status, Task, TaskRepository, apply, decide,
};

pub struct Services<'a> {
    pub tasks: &'a dyn TaskRepository,
    pub links: &'a dyn LinkRepository,
    pub collections: &'a dyn CollectionRepository,
    pub clock: &'a dyn Clock,
    pub ids: &'a dyn IdGenerator,
}

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From, PartialEq)]
pub enum Error {
    #[from(skip)]
    #[display("task not found: {_0}")]
    NotFound(#[error(not(source))] Id),
    #[display("denied: {_0}")]
    Denied(Denied),
    #[from(skip)]
    #[display("would create a cycle: {_0}")]
    Cycle(#[error(not(source))] String),
    #[from(skip)]
    #[display("import error: {_0}")]
    Import(#[error(not(source))] String),
}

impl<'a> Services<'a> {
    pub fn load(&self, id: &Id) -> Result<Task, Error> {
        self.tasks
            .get(id)
            .ok_or_else(|| Error::NotFound(id.clone()))
    }

    /// Child links out of `parent`, ordered by position.
    pub fn children_of(&self, parent: &Id) -> Vec<tda_core::Link> {
        self.links.outgoing(parent, LinkKind::Child)
    }

    /// The structural parent of `child`, if any (single-parent tree).
    pub fn parent_of(&self, child: &Id) -> Option<Id> {
        self.links
            .incoming(child, LinkKind::Child)
            .into_iter()
            .next()
            .map(|l| l.from)
    }

    /// Derived `blocked` (spec §8): some incoming `blocks` edge whose blocker
    /// task is not `done`.
    pub fn is_blocked(&self, id: &Id) -> bool {
        self.links.incoming(id, LinkKind::Blocks).iter().any(|l| {
            self.tasks
                .get(&l.from)
                .is_some_and(|b| b.status != Status::Done)
        })
    }

    /// All descendants of `id` via `child` links (excludes `id`).
    pub fn descendants(&self, id: &Id) -> HashSet<Id> {
        let mut seen = HashSet::new();
        let mut stack: Vec<Id> = self.children_of(id).into_iter().map(|l| l.to).collect();
        while let Some(cur) = stack.pop() {
            if seen.insert(cur.clone()) {
                stack.extend(self.children_of(&cur).into_iter().map(|l| l.to));
            }
        }
        seen
    }

    /// Run a task-local command through `decide → apply → persist` (spec §5a).
    pub fn run(&self, id: &Id, cmd: Command) -> Result<Task, Error> {
        let mut task = self.load(id)?;
        let ctx = DecideCtx {
            blocked: self.is_blocked(id),
        };
        let events = decide(&task, &cmd, &ctx)?;
        for e in &events {
            apply(&mut task, e);
        }
        if !events.is_empty() {
            task.updated_at = self.clock.now();
        }
        self.tasks.put(task.clone());
        Ok(task)
    }
}
