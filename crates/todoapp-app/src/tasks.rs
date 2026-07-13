//! Capture, edit, structure, and delegation use cases (FR-1..FR-12).

use std::collections::{BTreeMap, BTreeSet};

use todoapp_core::{
    Attachment, AttachmentKind, Attachments, Command, ComponentStore, Date, Due, Duration, Id,
    IssueRef, Link, LinkKind, Position, Recurrence, Status, Tags, TaskEntityStore, Title,
    Workspace, extract_title_syntax,
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
        let extracted = extract_title_syntax(&title.into());
        let id = self.ids.next_id();
        let now = self.clock.now();
        self.store.create(&id, now, now).await;
        self.store.set(&id, Title(extracted.title)).await;
        self.store.set(&id, status).await;
        let mut tags: BTreeSet<String> = tags.into_iter().collect();
        tags.extend(extracted.tags);
        if !tags.is_empty() {
            self.store.set(&id, Tags(tags)).await;
        }
        // Top-level tasks attach under the virtual-root sentinel (spec §7), so a
        // root is just another `child` edge and `roots()` is a plain port query.
        let root = Id::root();
        self.attach(&id, parent.unwrap_or(&root), None).await?;
        // `@name`/`#tag`/`[...]` title syntax (spec FR-32/FR-33/FR-34):
        // additive, idempotent (Assign is a no-op if already assigned).
        for actor in extracted.mentions {
            self.assign(&id, actor).await?;
        }
        if let Some(due) = extracted.due {
            self.set_due(&id, Some(due.resolve(self.clock.today())))
                .await?;
        }
        if let Some(recurrence) = extracted.recurrence {
            self.set_recurrence(&id, Some(recurrence)).await?;
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
        let extracted = extract_title_syntax(&title.into());
        let snap = self.run(id, Command::SetTitle(extracted.title)).await?;
        if extracted.mentions.is_empty()
            && extracted.tags.is_empty()
            && extracted.due.is_none()
            && extracted.recurrence.is_none()
        {
            return Ok(snap);
        }
        for actor in extracted.mentions {
            self.assign(id, actor).await?;
        }
        for tag in extracted.tags {
            self.add_tag(id, tag).await?;
        }
        if let Some(due) = extracted.due {
            self.set_due(id, Some(due.resolve(self.clock.today())))
                .await?;
        }
        if let Some(recurrence) = extracted.recurrence {
            self.set_recurrence(id, Some(recurrence)).await?;
        }
        self.snapshot(id).await
    }
    pub async fn set_notes(&self, id: &Id, notes: Option<String>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetNotes(notes)).await
    }
    pub async fn set_status(&self, id: &Id, status: Status) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetStatus(status)).await
    }
    pub async fn set_due(&self, id: &Id, due: Option<Due>) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetSchedule(due)).await
    }
    pub async fn set_estimate(
        &self,
        id: &Id,
        estimate: Option<Duration>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetEstimate(estimate)).await
    }
    pub async fn add_time_spent(&self, id: &Id, spent: Duration) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::AddTimeSpent(spent)).await
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
    /// A recurring task resets in place on completion instead of staying
    /// `done` (see `Event::StatusSet(Status::Done)`'s apply arm).
    pub async fn set_recurrence(
        &self,
        id: &Id,
        recurrence: Option<Recurrence>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetRecurrence(recurrence)).await
    }
    pub async fn set_issue_ref(
        &self,
        id: &Id,
        issue_ref: Option<IssueRef>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetIssueRef(issue_ref)).await
    }
    pub async fn set_workspace(
        &self,
        id: &Id,
        workspace: Option<Workspace>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetWorkspace(workspace)).await
    }
    pub async fn set_time_log(
        &self,
        id: &Id,
        time_log: BTreeMap<Date, Duration>,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetTimeLog(time_log)).await
    }
    pub async fn set_archived(&self, id: &Id, archived: bool) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::SetArchived(archived)).await
    }
    /// Attach a file's actual bytes (stored via `BlobStore`) to a task.
    pub async fn add_attachment_from_bytes(
        &self,
        id: &Id,
        title: impl Into<String>,
        bytes: Vec<u8>,
        mime: Option<String>,
    ) -> Result<TaskSnapshot, Error> {
        let blob = self.blobs.put(bytes).await;
        let att = Attachment {
            id: self.ids.next_id(),
            kind: AttachmentKind::File,
            title: title.into(),
            url: None,
            blob: Some(blob),
            mime,
        };
        self.run(id, Command::AddAttachment(att)).await
    }
    /// Attach a bare link (no stored bytes) to a task.
    pub async fn add_attachment_link(
        &self,
        id: &Id,
        title: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<TaskSnapshot, Error> {
        let att = Attachment {
            id: self.ids.next_id(),
            kind: AttachmentKind::Link,
            title: title.into(),
            url: Some(url.into()),
            blob: None,
            mime: None,
        };
        self.run(id, Command::AddAttachment(att)).await
    }
    pub async fn remove_attachment(
        &self,
        id: &Id,
        attachment_id: Id,
    ) -> Result<TaskSnapshot, Error> {
        self.run(id, Command::RemoveAttachment(attachment_id)).await
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
        if let Some(old) = self.raw_parent_of(id).await {
            self.links.remove(&old, id, LinkKind::Child).await;
        }
        self.attach(id, parent, anchor).await
    }

    /// Reorder `id` among its existing siblings (FR-7).
    pub async fn reorder(&self, id: &Id, anchor: Anchor) -> Result<(), Error> {
        // raw parent so a top-level task reorders among the sentinel's children.
        let parent = self
            .raw_parent_of(id)
            .await
            .ok_or_else(|| Error::Cycle(format!("{id} has no parent to reorder within")))?;
        self.attach(id, &parent, Some(anchor)).await
    }

    /// Delete `id` and all its capability components. Rejects if `id` has
    /// children unless `recursive` (then deletes the whole subtree,
    /// deepest-first). Cleans up child/blocks links and removes any
    /// attachment blob no longer referenced by another task.
    pub async fn delete_task(&self, id: &Id, recursive: bool) -> Result<(), Error> {
        if self.store.meta(id).await.is_none() {
            return Err(Error::NotFound(id.clone()));
        }
        let mut victims = vec![id.clone()];
        let descendants = self.descendants(id).await;
        if !descendants.is_empty() {
            if !recursive {
                return Err(Error::Cycle(format!("{id} has children; use --recursive")));
            }
            victims.extend(descendants);
        }
        // Deepest-first isn't required for correctness (component/link removal
        // is independent per id), but delete children before parents so a crash
        // mid-way never leaves a parent pointing at an already-gone child.
        for victim in victims.iter().rev() {
            self.delete_one(victim).await;
        }
        Ok(())
    }

    /// Remove one task's components, its child/blocks links (both
    /// directions), and any attachment blob not referenced by a surviving
    /// task.
    async fn delete_one(&self, id: &Id) {
        if let Some(attachments) = self.store.get::<Attachments>(id).await {
            for att in attachments.0 {
                if let Some(blob) = att.blob
                    && !self.blob_in_use_elsewhere(&blob, id).await
                {
                    self.blobs.remove(&blob).await;
                }
            }
        }
        if let Some(parent) = self.raw_parent_of(id).await {
            self.links.remove(&parent, id, LinkKind::Child).await;
        }
        for child in self.children_of(id).await {
            self.links.remove(id, &child.to, LinkKind::Child).await;
        }
        for l in self.links.incoming(id, LinkKind::Blocks).await {
            self.links.remove(&l.from, id, LinkKind::Blocks).await;
        }
        for l in self.links.outgoing(id, LinkKind::Blocks).await {
            self.links.remove(id, &l.to, LinkKind::Blocks).await;
        }
        self.store.delete(id).await;
    }

    /// Does any task other than `excluding` still reference `blob` in its
    /// `Attachments`? ponytail: O(n) scan over every task id — fine for a
    /// single-user/local tool; revisit only if attachment counts get large.
    async fn blob_in_use_elsewhere(&self, blob: &Id, excluding: &Id) -> bool {
        for other in self.store.all().await {
            if &other == excluding {
                continue;
            }
            if let Some(atts) = self.store.get::<Attachments>(&other).await
                && atts.0.iter().any(|a| a.blob.as_ref() == Some(blob))
            {
                return true;
            }
        }
        false
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
