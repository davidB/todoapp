//! Capture, edit, structure, and delegation use cases (FR-1..FR-12).

use tda_core::{Command, Id, Link, LinkKind, Position, Status, Task};

use crate::service::{Error, Services};

/// Where to drop a task among its new siblings.
pub enum Anchor {
    Before(Id),
    After(Id),
}

impl<'a> Services<'a> {
    /// FR-1/FR-2: create one task, optionally under `parent` (appended last).
    pub fn create(
        &self,
        title: impl Into<String>,
        parent: Option<&Id>,
        status: Status,
        tags: impl IntoIterator<Item = String>,
    ) -> Result<Task, Error> {
        let mut task = Task::new(self.ids.next_id(), title, status, self.clock.now());
        task.tags = tags.into_iter().collect();
        let id = task.id.clone();
        self.tasks.put(task.clone());
        if let Some(p) = parent {
            self.attach(&id, p, None)?;
        }
        Ok(task)
    }

    /// FR-1: batch-create from text, indentation (2 spaces or a tab) = depth
    /// (spec §13 Q7 default). Returns tasks in document order.
    pub fn batch_create(&self, text: &str) -> Result<Vec<Task>, Error> {
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
            let task = self.create(title, parent.as_ref(), Status::Draft, [])?;
            stack.push(task.id.clone());
            created.push(task);
        }
        Ok(created)
    }

    // ---- task-local edits (thin wrappers over the decider) ----------------

    pub fn set_title(&self, id: &Id, title: impl Into<String>) -> Result<Task, Error> {
        self.run(id, Command::SetTitle(title.into()))
    }
    pub fn set_notes(&self, id: &Id, notes: Option<String>) -> Result<Task, Error> {
        self.run(id, Command::SetNotes(notes))
    }
    pub fn set_status(&self, id: &Id, status: Status) -> Result<Task, Error> {
        self.run(id, Command::SetStatus(status))
    }
    pub fn set_due(&self, id: &Id, due: Option<String>) -> Result<Task, Error> {
        self.run(id, Command::SetSchedule(due))
    }
    pub fn set_estimate(&self, id: &Id, minutes: Option<u32>) -> Result<Task, Error> {
        self.run(id, Command::SetEstimate(minutes))
    }
    pub fn add_time_spent(&self, id: &Id, minutes: u32) -> Result<Task, Error> {
        self.run(id, Command::AddTimeSpent(minutes))
    }
    pub fn add_tag(&self, id: &Id, tag: impl Into<String>) -> Result<Task, Error> {
        self.run(id, Command::AddTag(tag.into()))
    }
    pub fn remove_tag(&self, id: &Id, tag: impl Into<String>) -> Result<Task, Error> {
        self.run(id, Command::RemoveTag(tag.into()))
    }
    pub fn assign(&self, id: &Id, actor: Id) -> Result<Task, Error> {
        self.run(id, Command::Assign(actor))
    }
    pub fn unassign(&self, id: &Id, actor: Id) -> Result<Task, Error> {
        self.run(id, Command::Unassign(actor))
    }
    /// FR-11: claim a `todo` task (open if unassigned, else assignee-only).
    pub fn claim(&self, id: &Id, actor: Id) -> Result<Task, Error> {
        self.run(id, Command::Claim(actor))
    }

    // ---- structure (FR-4..FR-8): graph-aware, validated here, not in decide -

    /// Re-point `id`'s single `child` parent to `parent` at `anchor` (FR-8).
    /// Rejects a move under the task's own subtree (cycle).
    pub fn move_task(&self, id: &Id, parent: &Id, anchor: Option<Anchor>) -> Result<(), Error> {
        if parent == id || self.descendants(id).contains(parent) {
            return Err(Error::Cycle(format!("{id} cannot be moved under {parent}")));
        }
        if let Some(old) = self.parent_of(id) {
            self.links.remove(&old, id, LinkKind::Child);
        }
        self.attach(id, parent, anchor)
    }

    /// Reorder `id` among its existing siblings (FR-7).
    pub fn reorder(&self, id: &Id, anchor: Anchor) -> Result<(), Error> {
        let parent = self
            .parent_of(id)
            .ok_or_else(|| Error::Cycle(format!("{id} has no parent to reorder within")))?;
        self.attach(id, &parent, Some(anchor))
    }

    /// FR-6: add a `blocks` edge `blocker → blocked`; rejects a new cycle.
    pub fn block(&self, blocker: &Id, blocked: &Id) -> Result<(), Error> {
        if blocker == blocked || self.blocks_reaches(blocked, blocker) {
            return Err(Error::Cycle(format!("{blocker} blocks {blocked}")));
        }
        let last = self
            .links
            .outgoing(blocker, LinkKind::Blocks)
            .last()
            .map(|l| l.position.0);
        self.links.put(Link {
            from: blocker.clone(),
            to: blocked.clone(),
            kind: LinkKind::Blocks,
            position: Position(Position::between(last, None)),
        });
        Ok(())
    }

    /// Add/replace the `child` link `parent → id`, positioned per `anchor`.
    pub(crate) fn attach(&self, id: &Id, parent: &Id, anchor: Option<Anchor>) -> Result<(), Error> {
        // siblings excluding the one being (re)positioned
        let sibs: Vec<_> = self
            .children_of(parent)
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
        self.links.put(Link {
            from: parent.clone(),
            to: id.clone(),
            kind: LinkKind::Child,
            position: Position(pos),
        });
        Ok(())
    }

    /// Can `start` reach `target` following `blocks` edges? (cycle test)
    fn blocks_reaches(&self, start: &Id, target: &Id) -> bool {
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
