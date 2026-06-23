//! Export/import a branch (FR-16/FR-17). JSON round-trips exactly; Markdown is
//! the human/agent task-list form (title + checkbox status + indentation depth).

use serde::{Deserialize, Serialize};
use tda_core::{Id, Link, LinkKind, Status, Task};

use crate::service::{Error, Services};

/// Self-contained snapshot of a branch: its tasks and the `child`/`blocks` edges
/// among them. Deterministically ordered so `export → import → export` is stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub tasks: Vec<Task>,
    pub links: Vec<Link>,
}

impl<'a> Services<'a> {
    /// Collect `root` + its descendant subtree (tasks and the edges among them).
    pub fn export(&self, root: &Id) -> Result<Export, Error> {
        let mut ids = self.descendants(root);
        ids.insert(root.clone());

        let mut tasks: Vec<Task> = ids.iter().filter_map(|id| self.tasks.get(id)).collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));

        let mut links = Vec::new();
        for id in &ids {
            for kind in [LinkKind::Child, LinkKind::Blocks] {
                for l in self.links.outgoing(id, kind) {
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

    pub fn export_json(&self, root: &Id) -> Result<String, Error> {
        let export = self.export(root)?;
        serde_json::to_string_pretty(&export).map_err(|e| Error::Import(e.to_string()))
    }

    /// FR-17: ingest an `Export` (round-trips with [`Self::export`]).
    pub fn import_json(&self, json: &str) -> Result<(), Error> {
        let export: Export =
            serde_json::from_str(json).map_err(|e| Error::Import(e.to_string()))?;
        for task in export.tasks {
            self.tasks.put(task);
        }
        for link in export.links {
            self.links.put(link);
        }
        Ok(())
    }

    /// FR-16: Markdown task list of a branch (DFS over `child`, position order).
    pub fn export_md(&self, root: &Id) -> Result<String, Error> {
        let mut out = String::new();
        self.md_node(root, 0, &mut out)?;
        Ok(out)
    }

    fn md_node(&self, id: &Id, depth: usize, out: &mut String) -> Result<(), Error> {
        let task = self.load(id)?;
        let mark = if task.status == Status::Done {
            "x"
        } else {
            " "
        };
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("- [{mark}] {}\n", task.title));
        for link in self.children_of(id) {
            self.md_node(&link.to, depth + 1, out)?;
        }
        Ok(())
    }

    /// FR-17: parse a Markdown task list into a tree (indent = depth). Status
    /// comes from the checkbox (`[x]` → done, else todo). Returns the roots.
    pub fn import_md(&self, md: &str) -> Result<Vec<Task>, Error> {
        let mut roots = Vec::new();
        let mut stack: Vec<Id> = Vec::new();
        for raw in md.lines() {
            let Some((depth, mark, title)) = parse_md_line(raw) else {
                continue;
            };
            stack.truncate(depth);
            let parent = stack.last().cloned();
            let status = if mark == 'x' {
                Status::Done
            } else {
                Status::Todo
            };
            let task = self.create(title, parent.as_ref(), status, [])?;
            if parent.is_none() {
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
