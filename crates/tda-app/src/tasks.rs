//! Capture, edit, structure, and delegation use cases (FR-1..FR-12).

use std::collections::BTreeSet;

use tda_core::{
    Command, ComponentStore, Id, Link, LinkKind, Position, Status, Tags, TaskEntityStore, Title,
};

use crate::service::{Error, Services, TaskSnapshot};

/// Where to drop a task among its new siblings.
pub enum Anchor {
    Before(Id),
    After(Id),
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    /// FR-1/FR-2: create one task, optionally under `parent` (appended last).
    pub async fn create(
        &self,
        title: impl Into<String>,
        parent: Option<&Id>,
        status: Status,
        tags: impl IntoIterator<Item = String>,
    ) -> Result<TaskSnapshot, Error> {
        let id = self.ids.next_id();
        let now = self.clock.now();
        self.store.create(&id, now, now).await;
        self.store.set(&id, Title(title.into())).await;
        self.store.set(&id, status).await;
        let tags: BTreeSet<String> = tags.into_iter().collect();
        if !tags.is_empty() {
            self.store.set(&id, Tags(tags)).await;
        }
        if let Some(p) = parent {
            self.attach(&id, p, None).await?;
        }
        self.snapshot(&id).await
    }

    /// FR-1: batch-create from text, indentation (2 spaces or a tab) = depth
    /// (spec §13 Q7 default). Returns tasks in document order.
    pub async fn batch_create(&self, text: &str) -> Result<Vec<TaskSnapshot>, Error> {
        let mut created = Vec::new();
        // stack[d] = id of the most recent task at depth d (its children sit at d+1).
        let mut stack: Vec<Id> = Vec::new();
        for raw in text.lines() {
            let title = raw.trim();
            if title.is_empty() {
                continue;
            }
            let depth = indent_depth(raw);
            stack.truncate(depth);
            let parent = stack.last().cloned();
            let task = self
                .create(title, parent.as_ref(), Status::Draft, [])
                .await?;
            stack.push(task.id.clone());
            created.push(task);
        }
        Ok(created)
    }

    // ---- task-local edits (thin wrappers over the decider) ----------------

    pub async fn set_title(
        &self,
        id: &Id,
        title: impl Into<String>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetTitle(title.into())).await
    }
    pub async fn set_notes(&self, id: &Id, notes: Option<String>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetNotes(notes)).await
    }
    pub async fn set_status(&self, id: &Id, status: Status) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetStatus(status)).await
    }
    pub async fn set_due(&self, id: &Id, due: Option<String>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetSchedule(due)).await
    }
    pub async fn set_estimate(&self, id: &Id, minutes: Option<u32>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetEstimate(minutes)).await
    }
    pub async fn add_time_spent(&self, id: &Id, minutes: u32) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::AddTimeSpent(minutes)).await
    }
    pub async fn add_tag(&self, id: &Id, tag: impl Into<String>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::AddTag(tag.into())).await
    }
    pub async fn remove_tag(&self, id: &Id, tag: impl Into<String>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::RemoveTag(tag.into())).await
    }
    pub async fn assign(&self, id: &Id, actor: Id) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::Assign(actor)).await
    }
    pub async fn unassign(&self, id: &Id, actor: Id) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::Unassign(actor)).await
    }
    /// FR-11: claim a `todo` task (open if unassigned, else assignee-only).
    pub async fn claim(&self, id: &Id, actor: Id) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::Claim(actor)).await
    }

    // ---- structure (FR-4..FR-8): graph-aware, validated here, not in decide -

    /// Re-point `id`'s single `child` parent to `parent` at `anchor` (FR-8).
    /// Rejects a move under the task's own subtree (cycle).
    pub async fn move_task(
        &self,
        id: &Id,
        parent: &Id,
        anchor: Option<Anchor>,
    ) -> Result<(), Error> {
        if parent == id || self.descendants(id).await.contains(parent) {
            return Err(Error::Cycle(format!("{id} cannot be moved under {parent}")));
        }
        if let Some(old) = self.parent_of(id).await {
            self.links.remove(&old, id, LinkKind::Child).await;
        }
        self.attach(id, parent, anchor).await
    }

    /// Reorder `id` among its existing siblings (FR-7).
    pub async fn reorder(&self, id: &Id, anchor: Anchor) -> Result<(), Error> {
        let parent = self
            .parent_of(id)
            .await
            .ok_or_else(|| Error::Cycle(format!("{id} has no parent to reorder within")))?;
        self.attach(id, &parent, Some(anchor)).await
    }

    /// FR-6: add a `blocks` edge `blocker → blocked`; rejects a new cycle.
    pub async fn block(&self, blocker: &Id, blocked: &Id) -> Result<(), Error> {
        if blocker == blocked || self.blocks_reaches(blocked, blocker).await {
            return Err(Error::Cycle(format!("{blocker} blocks {blocked}")));
        }
        let last = self
            .links
            .outgoing(blocker, LinkKind::Blocks)
            .await
            .last()
            .map(|l| l.position.0);
        self.links
            .put(Link {
                from: blocker.clone(),
                to: blocked.clone(),
                kind: LinkKind::Blocks,
                position: Position(Position::between(last, None)),
            })
            .await;
        Ok(())
    }

    /// Add/replace the `child` link `parent → id`, positioned per `anchor`.
    pub(crate) async fn attach(
        &self,
        id: &Id,
        parent: &Id,
        anchor: Option<Anchor>,
    ) -> Result<(), Error> {
        // siblings excluding the one being (re)positioned
        let sibs: Vec<_> = self
            .children_of(parent)
            .await
            .into_iter()
            .filter(|l| &l.to != id)
            .collect();
        let pos = match anchor {
            None => Position::between(sibs.last().map(|l| l.position.0), None),
            Some(Anchor::Before(x)) => {
                let i = sibs.iter().position(|l| l.to == x);
                match i {
                    Some(i) => {
                        let before = if i == 0 {
                            None
                        } else {
                            Some(sibs[i - 1].position.0)
                        };
                        Position::between(before, Some(sibs[i].position.0))
                    }
                    None => Position::between(sibs.last().map(|l| l.position.0), None),
                }
            }
            Some(Anchor::After(x)) => {
                let i = sibs.iter().position(|l| l.to == x);
                match i {
                    Some(i) => {
                        let after = sibs.get(i + 1).map(|l| l.position.0);
                        Position::between(Some(sibs[i].position.0), after)
                    }
                    None => Position::between(sibs.last().map(|l| l.position.0), None),
                }
            }
        };
        self.links
            .put(Link {
                from: parent.clone(),
                to: id.clone(),
                kind: LinkKind::Child,
                position: Position(pos),
            })
            .await;
        Ok(())
    }

    /// Can `start` reach `target` following `blocks` edges? (cycle test)
    async fn blocks_reaches(&self, start: &Id, target: &Id) -> bool {
        let mut stack = vec![start.clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            if &cur == target {
                return true;
            }
            if seen.insert(cur.clone()) {
                stack.extend(
                    self.links
                        .outgoing(&cur, LinkKind::Blocks)
                        .await
                        .into_iter()
                        .map(|l| l.to),
                );
            }
        }
        false
    }
}

fn indent_depth(line: &str) -> usize {
    let mut spaces = 0;
    for c in line.chars() {
        match c {
            '\t' => spaces += 2,
            ' ' => spaces += 1,
            _ => break,
        }
    }
    spaces / 2
}
