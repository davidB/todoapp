//! Export/import a branch (FR-16/FR-17). JSON round-trips exactly; Markdown is
//! the human/agent task-list form (title + checkbox status + indentation depth).

use serde::{Deserialize, Serialize};
use todoapp_core::{ComponentStore, Id, Link, LinkKind, Status, TaskEntityStore};

use crate::service::{Error, Services, TaskSnapshot};

/// Self-contained snapshot of a branch: its tasks and the `child`/`blocks` edges
/// among them. Deterministically ordered so `export → import → export` is stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub tasks: Vec<TaskSnapshot>,
    pub links: Vec<Link>,
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    /// Collect `root` + its descendant subtree (tasks and the edges among them).
    pub async fn export(&self, root: &Id) -> Result<Export, Error> {
        let mut ids = self.descendants(root).await;
        ids.insert(root.clone());

        let mut tasks: Vec<TaskSnapshot> = Vec::new();
        for id in &ids {
            if let Ok(t) = self.snapshot(id).await {
                tasks.push(t);
            }
        }
        tasks.sort_by(|a, b| a.id.cmp(&b.id));

        let mut links = Vec::new();
        for id in &ids {
            for kind in [LinkKind::Child, LinkKind::Blocks] {
                for l in self.links.outgoing(id, kind).await {
                    if ids.contains(&l.to) {
                        links.push(l);
                    }
                }
            }
        }
        links.sort_by(|a, b| {
            (a.from.as_str(), a.to.as_str(), format!("{:?}", a.kind)).cmp(&(
                b.from.as_str(),
                b.to.as_str(),
                format!("{:?}", b.kind),
            ))
        });
        Ok(Export { tasks, links })
    }

    pub async fn export_json(&self, root: &Id) -> Result<String, Error> {
        let export = self.export(root).await?;
        serde_json::to_string_pretty(&export).map_err(|e| Error::Import(e.to_string()))
    }

    /// FR-17: ingest an `Export` (round-trips with [`Self::export`] when
    /// `parent` is `None`). Branch roots (no parent in the payload) attach
    /// under `parent`, or the virtual root if `None`.
    pub async fn import_json(&self, json: &str, parent: Option<&Id>) -> Result<(), Error> {
        let export: Export =
            serde_json::from_str(json).map_err(|e| Error::Import(e.to_string()))?;
        for task in &export.tasks {
            self.write_snapshot(task).await;
        }
        // Tasks with a `child` parent inside the payload (computed over the
        // import, not the whole store).
        let parented: std::collections::HashSet<&Id> = export
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Child)
            .map(|l| &l.to)
            .collect();
        for link in &export.links {
            self.links.put(link.clone()).await;
        }
        let attach_under = parent.cloned().unwrap_or_else(Id::root);
        for task in &export.tasks {
            if !parented.contains(&task.id) {
                self.attach(&task.id, &attach_under, None).await?;
            }
        }
        Ok(())
    }

    /// FR-12: prompt-ready Markdown context for working `id`: the ancestor
    /// chain (root → parent, titles + notes), the task itself in full, its
    /// direct children, and the inherited workspace. Self-contained on purpose
    /// — an agent can start a fresh session from this output alone.
    /// `overrides` maps workspace name → per-machine local path (from config).
    pub async fn context_md(
        &self,
        id: &Id,
        overrides: &std::collections::BTreeMap<String, String>,
    ) -> Result<String, Error> {
        let task = self.snapshot(id).await?;
        let mut ancestors = Vec::new();
        let mut cur = self.parent_of(id).await;
        while let Some(pid) = cur {
            cur = self.parent_of(&pid).await;
            ancestors.push(self.snapshot(&pid).await?);
        }
        ancestors.reverse();

        let mut out = format!("# Task: {} (`{}`)\n\n", task.title, task.id);
        if let Some(w) = self.workspace_of(id).await {
            out.push_str(&format!("Workspace: {}", w.name));
            let effective = overrides.get(&w.name).or(w.path.as_ref());
            if let Some(p) = effective {
                out.push_str(&format!(" — `{p}`"));
            }
            out.push('\n');
        }
        out.push_str(&format!("Status: {}", task.status));
        if self.is_blocked(id).await {
            out.push_str(" (blocked: a blocker is not done)");
        }
        out.push('\n');
        if !task.tags.is_empty() {
            let tags: Vec<&str> = task.tags.iter().map(String::as_str).collect();
            out.push_str(&format!("Tags: {}\n", tags.join(", ")));
        }
        if !task.assignments.is_empty() {
            let who: Vec<String> = task
                .assignments
                .iter()
                .map(|a| {
                    if a.claimed {
                        format!("{} (claimed)", a.actor)
                    } else {
                        a.actor.to_string()
                    }
                })
                .collect();
            out.push_str(&format!("Assignees: {}\n", who.join(", ")));
        }
        if let Some(d) = task.due_date {
            out.push_str(&format!("Due: {d}\n"));
        }
        if let Some(n) = &task.notes {
            out.push_str(&format!("\n{n}\n"));
        }

        if !ancestors.is_empty() {
            out.push_str("\n## Ancestors (root → parent)\n");
            for a in &ancestors {
                out.push_str(&format!("\n### {} (`{}`, {})\n", a.title, a.id, a.status));
                if let Some(n) = &a.notes {
                    out.push_str(&format!("\n{n}\n"));
                }
            }
        }

        let children = self.children_of(id).await;
        if !children.is_empty() {
            out.push_str("\n## Children\n\n");
            for l in children {
                if let Ok(c) = self.snapshot(&l.to).await {
                    out.push_str(&format!("- {} (`{}`, {})\n", c.title, c.id, c.status));
                }
            }
        }
        Ok(out)
    }

    /// FR-16: Markdown task list of a branch (DFS over `child`, position order).
    /// Iterative DFS (explicit stack) to avoid boxing an async recursion.
    pub async fn export_md(&self, root: &Id) -> Result<String, Error> {
        let mut out = String::new();
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((id, depth)) = stack.pop() {
            let task = self.snapshot(&id).await?;
            let mark = if task.status == Status::Done {
                "x"
            } else {
                " "
            };
            out.push_str(&"  ".repeat(depth));
            out.push_str(&format!("- [{mark}] {}\n", task.title));
            // push children in reverse so the first sibling is visited next
            for link in self.children_of(&id).await.into_iter().rev() {
                stack.push((link.to, depth + 1));
            }
        }
        Ok(out)
    }

    /// FR-17: parse a Markdown task list into a tree (indent = depth). Status
    /// comes from the checkbox (`[x]` → done, else todo). Top-level items
    /// (depth 0) attach under `parent`, or the virtual root if `None`.
    /// Returns the top-level tasks.
    pub async fn import_md(
        &self,
        md: &str,
        parent: Option<&Id>,
    ) -> Result<Vec<TaskSnapshot>, Error> {
        let mut roots = Vec::new();
        let mut stack: Vec<Id> = Vec::new();
        for raw in md.lines() {
            let Some((depth, mark, title)) = parse_md_line(raw) else {
                continue;
            };
            stack.truncate(depth);
            let task_parent = stack.last().cloned().or_else(|| parent.cloned());
            let status = if mark == 'x' {
                Status::Done
            } else {
                Status::Todo
            };
            let task = self.create(title, task_parent.as_ref(), status, []).await?;
            if depth == 0 {
                roots.push(task.clone());
            }
            stack.push(task.id.clone());
        }
        Ok(roots)
    }
}

/// `(depth, checkbox char, title)` for a `- [ ] ...` line; `None` for others.
fn parse_md_line(line: &str) -> Option<(usize, char, &str)> {
    let indent = line.len() - line.trim_start().len();
    let depth = indent / 2;
    let rest = line.trim_start();
    let rest = rest.strip_prefix("- [")?;
    let mark = rest.chars().next()?;
    let title = rest.get(1..)?.strip_prefix("] ")?.trim();
    Some((depth, mark, title))
}
