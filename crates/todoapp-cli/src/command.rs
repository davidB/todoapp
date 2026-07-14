//! CLI command model (`Cmd`) and its dispatch (`run_command`).
//!
//! `run_command` is shared by the headless `tda` path (direct against the DB)
//! and the TUI's IPC server (which runs commands sent by other `tda` processes).
//! It writes all output into a buffer (`Reply.out`) so the same code can print
//! to stdout or ship the bytes back over the socket. Path- and stdin-dependent
//! commands resolve against the *client's* `req.cwd` / `req.stdin`, not the
//! running process's, so proxied commands behave as if run in the caller's shell.

use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use todoapp_app::{Anchor, Services, TaskSnapshot};
use todoapp_core::{
    ComponentStore, Dir, DueFilter, DueSpec, Filter, Id, Query, Recurrence, SortField, SortKey,
    Status, TaskEntityStore, Workspace,
};

use crate::ipc::{Reply, Request};

// ---- CLI arg types ----------------------------------------------------------

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize)]
pub enum StatusArg {
    Draft,
    Todo,
    Wip,
    Paused,
    Done,
}

impl From<StatusArg> for Status {
    fn from(s: StatusArg) -> Self {
        match s {
            StatusArg::Draft => Status::Draft,
            StatusArg::Todo => Status::Todo,
            StatusArg::Wip => Status::Wip,
            StatusArg::Paused => Status::Paused,
            StatusArg::Done => Status::Done,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize)]
pub enum DuePeriodArg {
    Today,
    Overdue,
}

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize)]
pub enum SortArg {
    Priority,
    Due,
    Created,
    Updated,
}

impl From<SortArg> for SortKey {
    fn from(s: SortArg) -> Self {
        SortKey {
            key: match s {
                SortArg::Priority => SortField::Priority,
                SortArg::Due => SortField::Due,
                SortArg::Created => SortField::Created,
                SortArg::Updated => SortField::Updated,
            },
            dir: Dir::Asc,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Serialize, Deserialize)]
pub enum FormatArg {
    Md,
    Json,
    /// Super Productivity JSON backup — import-only, must be requested
    /// explicitly (`--format sp`); auto-detection can't tell it apart from
    /// tda's own `--format json` by extension alone.
    #[value(alias = "superproductivity")]
    Sp,
}

// ---- Command model ----------------------------------------------------------

#[derive(Subcommand, Clone, Serialize, Deserialize)]
pub enum Cmd {
    /// Launch the interactive TUI.
    Tui,
    /// Create a task. Use --batch to read a tree from stdin (indent = depth).
    Add {
        /// Task title (omit with --batch).
        title: Option<String>,
        /// Parent task ID.
        #[arg(long)]
        parent: Option<String>,
        /// Tags (repeatable).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Initial status.
        #[arg(long, default_value = "draft")]
        status: StatusArg,
        /// Read a tree from stdin (one title per line, 2-space indent = depth).
        #[arg(long, conflicts_with = "title")]
        batch: bool,
    },
    /// List tasks. Defaults to top-level roots.
    Ls {
        /// Show children of this task (all roots if omitted).
        id: Option<String>,
        /// Full subtree, flat DFS order with depth field.
        #[arg(long)]
        tree: bool,
    },
    /// Move a task to a new parent.
    Mv {
        id: String,
        #[arg(long)]
        to: String,
        #[arg(long, conflicts_with = "after")]
        before: Option<String>,
        #[arg(long, conflicts_with = "before")]
        after: Option<String>,
    },
    /// Delete a task. Fails if it has children unless --recursive.
    Rm {
        id: String,
        #[arg(long)]
        recursive: bool,
    },
    /// Add a dependency edge between tasks.
    Link {
        from: String,
        to: String,
        /// Only "blocks" is supported.
        #[arg(long, default_value = "blocks")]
        kind: String,
    },
    /// Assign a task to an actor.
    Assign { id: String, actor: String },
    /// Claim a task as the given actor.
    Claim {
        id: String,
        #[arg(long = "as")]
        actor: String,
    },
    /// Edit one or more fields on a task.
    Set {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        status: Option<StatusArg>,
        /// Due date: YYYY-MM-DD, "YYYY-MM-DD HH:MM", HH:MM, a weekday name
        /// (next occurrence), or "none" to clear.
        #[arg(long)]
        due: Option<String>,
        /// Recurrence: "daily", "every N days", "every mon,wed,fri",
        /// "monthly", "every N months", or "none" to clear.
        #[arg(long)]
        recurrence: Option<String>,
    },
    /// Add one or more tags to a task.
    Tag { id: String, tags: Vec<String> },
    /// Show one task in full: snapshot + parent, breadcrumb, children,
    /// blocked, inherited workspace.
    Show { id: String },
    /// Print prompt-ready Markdown context for a task: ancestor titles+notes,
    /// the task itself, its children, and the workspace folder.
    Context { id: String },
    /// Append a timestamped progress note to a task's notes (never clobbers).
    Note {
        id: String,
        text: String,
        /// Author shown on the note (e.g. claude-code/sonnet-5).
        #[arg(long = "as", default_value = "me")]
        actor: String,
    },
    /// Free-text search across titles and notes.
    Find { text: String },
    /// Query tasks with filters.
    Q {
        #[arg(long = "status")]
        status: Vec<StatusArg>,
        #[arg(long = "as")]
        assignee: Option<String>,
        #[arg(long)]
        under: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        due: Option<DuePeriodArg>,
        #[arg(long)]
        sort: Vec<SortArg>,
        /// Scope to the workspace containing the current directory.
        #[arg(long, conflicts_with = "under")]
        here: bool,
    },
    /// What to work on next (status:todo, by priority).
    Next {
        #[arg(long = "as")]
        assignee: Option<String>,
        #[arg(long)]
        under: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        /// Only tasks the --as actor may claim: unassigned or assigned to
        /// them, and not blocked (FR-11).
        #[arg(long, requires = "assignee")]
        claimable: bool,
        /// Scope to the workspace containing the current directory.
        #[arg(long, conflicts_with = "under")]
        here: bool,
    },
    /// Tasks due today or overdue.
    Due { period: DuePeriodArg },
    /// Export a task subtree (default: all roots).
    Export {
        id: Option<String>,
        #[arg(long, default_value = "md")]
        format: FormatArg,
    },
    /// Import tasks from a file. Without --parent, wraps the import in a new
    /// root task named after the file (name + mtime date). --parent root
    /// attaches top-level items directly at the root; --parent <id> attaches
    /// them under that task.
    Import {
        file: PathBuf,
        #[arg(long)]
        format: Option<FormatArg>,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Attach a file's contents to a task.
    Attach { id: String, file: PathBuf },
    /// Database management.
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Workspace binding: task subtree ↔ project folder. Without a subcommand,
    /// prints the workspace root resolved for the current directory.
    Ws {
        #[command(subcommand)]
        cmd: Option<WsCmd>,
    },
}

#[derive(Subcommand, Clone, Serialize, Deserialize)]
pub enum WsCmd {
    /// Bind a folder (default: cwd) to a workspace root task — creates the
    /// root task unless --root points at an existing one. The stored path is
    /// only a default; override it per machine via the config's [workspaces]
    /// table (name = "/local/path").
    Init {
        /// Folder to bind (default: current directory).
        path: Option<PathBuf>,
        /// Workspace name (default: the folder's name).
        #[arg(long)]
        name: Option<String>,
        /// Mark this existing task as the workspace root instead of creating one.
        #[arg(long)]
        root: Option<String>,
    },
}

#[derive(Subcommand, Clone, Serialize, Deserialize)]
pub enum DbCmd {
    /// Create a local database at ./.tda/tda.db, used by any `tda` run in
    /// this directory or below (like `git init`). Idempotent.
    Init,
    /// Print the database path the current directory resolves to.
    Path,
}

// ---- Output helpers ----------------------------------------------------------

fn write_json(out: &mut Vec<u8>, v: &impl Serialize) -> anyhow::Result<()> {
    writeln!(out, "{}", serde_json::to_string_pretty(v)?)?;
    Ok(())
}

/// Resolves `Ls`'s optional `id` arg: that task's direct children, or all roots if omitted.
async fn children_of_or_roots<St: ComponentStore + TaskEntityStore>(
    svc: &Services<'_, St>,
    id: Option<String>,
) -> Vec<Id> {
    match id {
        Some(s) => svc
            .children_of(&Id::new(s))
            .await
            .into_iter()
            .map(|l| l.to)
            .collect(),
        None => svc.roots().await,
    }
}

/// One row in a flat DFS tree listing.
#[derive(Serialize)]
struct TreeLine {
    depth: usize,
    #[serde(flatten)]
    task: TaskSnapshot,
}

/// `show`'s one-stop read for agents: the snapshot plus everything needed to
/// navigate from it (parent for sibling-adds, children, derived blocked,
/// inherited workspace with the *effective* local path).
#[derive(Serialize)]
struct ShowOut {
    #[serde(flatten)]
    task: TaskSnapshot,
    parent: Option<Id>,
    breadcrumb: Vec<String>,
    children: Vec<ChildLine>,
    blocked: bool,
    workspace: Option<Workspace>,
}

#[derive(Serialize)]
struct ChildLine {
    id: Id,
    title: String,
    status: Status,
}

/// Inherited workspace with the per-machine config override applied to `path`.
async fn effective_workspace<St: ComponentStore + TaskEntityStore>(
    svc: &Services<'_, St>,
    id: &Id,
    overrides: &std::collections::BTreeMap<String, String>,
) -> Option<Workspace> {
    let mut w = svc.workspace_of(id).await?;
    if let Some(p) = overrides.get(&w.name) {
        w.path = Some(p.clone());
    }
    Some(w)
}

/// Resolve `--here`: the workspace root task containing `cwd` (the caller's).
async fn here_root<St: ComponentStore + TaskEntityStore>(
    svc: &Services<'_, St>,
    cwd: &Path,
) -> anyhow::Result<Id> {
    svc.workspace_root_for(cwd, &todoapp_config::workspace_overrides())
        .await
        .with_context(|| {
            format!(
                "no workspace found for {} (see `tda ws init`)",
                cwd.display()
            )
        })
}

/// Title for the default `import --parent` wrapper task: the file's name plus
/// its modification date (local time), e.g. "notes.md 2026-07-12".
fn dated_wrapper_title(file: &Path) -> anyhow::Result<String> {
    let modified = std::fs::metadata(file)
        .with_context(|| format!("read metadata for {}", file.display()))?
        .modified()
        .with_context(|| format!("read mtime for {}", file.display()))?;
    let date = jiff::Timestamp::try_from(modified)?
        .to_zoned(jiff::tz::TimeZone::system())
        .date();
    let name = file
        .file_name()
        .map_or_else(|| "import".into(), |n| n.to_string_lossy().into_owned());
    Ok(format!("{name} {date}"))
}

// ---- Dispatch ---------------------------------------------------------------

/// Run a parsed command against `svc`, capturing its output. Never panics on a
/// command error — those become `Reply.err`. `Tui`/`Db` are handled by the
/// caller before dispatch and must not reach here.
pub async fn run_command<St: ComponentStore + TaskEntityStore>(
    svc: &Services<'_, St>,
    req: &Request,
) -> Reply {
    let mut out = Vec::new();
    match dispatch(svc, req, &mut out).await {
        Ok(()) => Reply { out, err: None },
        Err(e) => Reply {
            out,
            err: Some(format!("{e:#}")),
        },
    }
}

#[allow(clippy::too_many_lines)]
async fn dispatch<St: ComponentStore + TaskEntityStore>(
    svc: &Services<'_, St>,
    req: &Request,
    out: &mut Vec<u8>,
) -> anyhow::Result<()> {
    match req.cmd.clone() {
        Cmd::Tui | Cmd::Db { .. } => unreachable!("handled by the caller before dispatch"),
        Cmd::Add {
            title,
            parent,
            tags,
            status,
            batch,
        } => {
            let parent_id = parent.as_deref().map(Id::new);
            if batch {
                let text = String::from_utf8(req.stdin.clone()).context("stdin was not UTF-8")?;
                let created = svc.batch_create(&text).await?;
                write_json(out, &created)?;
            } else {
                let title = title.context("title required unless --batch")?;
                let task = svc
                    .create(title, parent_id.as_ref(), status.into(), tags)
                    .await?;
                write_json(out, &task)?;
            }
        }

        Cmd::Ls { id, tree } => {
            if tree {
                let root_ids = children_of_or_roots(svc, id).await;
                let mut lines: Vec<TreeLine> = Vec::new();
                let mut stack: Vec<(Id, usize)> = root_ids.into_iter().map(|id| (id, 0)).collect();
                stack.reverse();
                while let Some((id, depth)) = stack.pop() {
                    if let Ok(task) = svc.snapshot(&id).await {
                        let children: Vec<Id> = svc
                            .children_of(&id)
                            .await
                            .into_iter()
                            .map(|l| l.to)
                            .collect();
                        for child in children.into_iter().rev() {
                            stack.push((child, depth + 1));
                        }
                        lines.push(TreeLine { depth, task });
                    }
                }
                write_json(out, &lines)?;
            } else {
                let ids = children_of_or_roots(svc, id).await;
                let mut tasks: Vec<TaskSnapshot> = Vec::new();
                for id in ids {
                    if let Ok(t) = svc.snapshot(&id).await {
                        tasks.push(t);
                    }
                }
                write_json(out, &tasks)?;
            }
        }

        Cmd::Mv {
            id,
            to,
            before,
            after,
        } => {
            let anchor = match (before, after) {
                (Some(b), _) => Some(Anchor::Before(Id::new(b))),
                (_, Some(a)) => Some(Anchor::After(Id::new(a))),
                _ => None,
            };
            svc.move_task(&Id::new(id), &Id::new(to), anchor).await?;
            write_json(out, &serde_json::json!({"ok": true}))?;
        }

        Cmd::Rm { id, recursive } => {
            svc.delete_task(&Id::new(id), recursive).await?;
            write_json(out, &serde_json::json!({"ok": true}))?;
        }

        Cmd::Link { from, to, kind } => {
            if kind != "blocks" {
                anyhow::bail!("only --kind blocks is supported");
            }
            svc.block(&Id::new(from), &Id::new(to)).await?;
            write_json(out, &serde_json::json!({"ok": true}))?;
        }

        Cmd::Assign { id, actor } => {
            let task = svc.assign(&Id::new(id), Id::new(actor)).await?;
            write_json(out, &task)?;
        }

        Cmd::Claim { id, actor } => {
            let task = svc.claim(&Id::new(id), Id::new(actor)).await?;
            write_json(out, &task)?;
        }

        Cmd::Set {
            id,
            title,
            notes,
            status,
            due,
            recurrence,
        } => {
            let id = Id::new(id);
            let mut task = svc.snapshot(&id).await?;
            if let Some(t) = title {
                task = svc.set_title(&id, t).await?;
            }
            if let Some(n) = notes {
                task = svc.set_notes(&id, Some(n)).await?;
            }
            if let Some(s) = status {
                task = svc.set_status(&id, s.into()).await?;
            }
            if let Some(d) = due {
                let val = if d == "none" {
                    None
                } else {
                    Some(
                        DueSpec::parse(&d)
                            .map_err(|e| anyhow::anyhow!(e))
                            .context(
                                "parse --due as YYYY-MM-DD, \"YYYY-MM-DD HH:MM\", HH:MM, or a weekday name",
                            )?
                            .resolve(svc.clock.today()),
                    )
                };
                task = svc.set_due(&id, val).await?;
            }
            if let Some(r) = recurrence {
                let val = if r == "none" {
                    None
                } else {
                    Some(
                        Recurrence::parse(&r)
                            .map_err(|e| anyhow::anyhow!(e))
                            .context(
                                "parse --recurrence as \"daily\", \"every N days\", \"every mon,wed,fri\", \"monthly\", or \"every N months\"",
                            )?,
                    )
                };
                task = svc.set_recurrence(&id, val).await?;
            }
            write_json(out, &task)?;
        }

        Cmd::Tag { id, tags } => {
            let id = Id::new(id);
            let mut task = svc.snapshot(&id).await?;
            for tag in tags {
                task = svc.add_tag(&id, tag).await?;
            }
            write_json(out, &task)?;
        }

        Cmd::Show { id } => {
            let id = Id::new(id);
            let task = svc.snapshot(&id).await?;
            let children = {
                let mut cs = Vec::new();
                for l in svc.children_of(&id).await {
                    if let Ok(c) = svc.snapshot(&l.to).await {
                        cs.push(ChildLine {
                            id: c.id,
                            title: c.title,
                            status: c.status,
                        });
                    }
                }
                cs
            };
            let show = ShowOut {
                parent: svc.parent_of(&id).await,
                breadcrumb: svc.breadcrumb(&id).await,
                children,
                blocked: svc.is_blocked(&id).await,
                workspace: effective_workspace(svc, &id, &todoapp_config::workspace_overrides())
                    .await,
                task,
            };
            write_json(out, &show)?;
        }

        Cmd::Context { id } => {
            let md = svc
                .context_md(&Id::new(id), &todoapp_config::workspace_overrides())
                .await?;
            write!(out, "{md}")?;
        }

        Cmd::Note { id, text, actor } => {
            let id = Id::new(id);
            let task = svc.snapshot(&id).await?;
            let stamp = jiff::Zoned::now().strftime("%Y-%m-%d %H:%M");
            let entry = format!("---\n{actor} {stamp}\n{text}");
            // ponytail: read-modify-write, single-user; make it a command if
            // concurrent writers ever matter.
            let notes = match task.notes {
                Some(n) => format!("{n}\n\n{entry}"),
                None => entry,
            };
            let task = svc.set_notes(&id, Some(notes)).await?;
            write_json(out, &task)?;
        }

        Cmd::Find { text } => {
            let hits = svc
                .evaluate(&Query {
                    filter: Filter {
                        text: Some(text),
                        ..Default::default()
                    },
                    sort: vec![],
                })
                .await;
            write_json(out, &hits.iter().map(|h| &h.task).collect::<Vec<_>>())?;
        }

        Cmd::Q {
            status,
            assignee,
            under,
            tag,
            due,
            sort,
            here,
        } => {
            let due_filter = due.map(|d| match d {
                DuePeriodArg::Today => DueFilter::Today,
                DuePeriodArg::Overdue => DueFilter::Overdue,
            });
            let within = if here {
                Some(here_root(svc, &req.cwd).await?)
            } else {
                under.map(Id::new)
            };
            let hits = svc
                .evaluate(&Query {
                    filter: Filter {
                        status: status.into_iter().map(Status::from).collect(),
                        assignee: assignee.map(Id::new),
                        within,
                        tags: tag,
                        due: due_filter,
                        ..Default::default()
                    },
                    sort: sort.into_iter().map(SortKey::from).collect(),
                })
                .await;
            write_json(out, &hits)?;
        }

        Cmd::Next {
            assignee,
            under,
            tag,
            claimable,
            here,
        } => {
            let within = if here {
                Some(here_root(svc, &req.cwd).await?)
            } else {
                under.map(Id::new)
            };
            let hits = if claimable {
                // clap's `requires` already enforces --as; context is belt-and-braces
                let actor = Id::new(assignee.context("--claimable requires --as")?);
                svc.claimable_for(&actor, within, tag).await
            } else {
                svc.what_next_for(assignee.map(Id::new), within, tag).await
            };
            write_json(out, &hits)?;
        }

        Cmd::Due { period } => {
            let hits = match period {
                DuePeriodArg::Today => svc.due_today().await,
                DuePeriodArg::Overdue => {
                    svc.evaluate(&Query {
                        filter: Filter {
                            due: Some(DueFilter::Overdue),
                            ..Default::default()
                        },
                        sort: vec![SortKey {
                            key: SortField::Due,
                            dir: Dir::Asc,
                        }],
                    })
                    .await
                }
            };
            write_json(out, &hits.iter().map(|h| &h.task).collect::<Vec<_>>())?;
        }

        Cmd::Export { id, format } => {
            let root = id.map(Id::new);
            match format {
                FormatArg::Md => {
                    let md = if let Some(id) = &root {
                        svc.export_md(id).await?
                    } else {
                        let mut s = String::new();
                        for root_id in svc.roots().await {
                            s.push_str(&svc.export_md(&root_id).await?);
                        }
                        s
                    };
                    write!(out, "{md}")?;
                }
                FormatArg::Json => {
                    let json = if let Some(id) = &root {
                        svc.export_json(id).await?
                    } else {
                        let mut all = todoapp_app::Export {
                            tasks: vec![],
                            links: vec![],
                        };
                        for root_id in svc.roots().await {
                            let e = svc.export(&root_id).await?;
                            all.tasks.extend(e.tasks);
                            all.links.extend(e.links);
                        }
                        serde_json::to_string_pretty(&all)?
                    };
                    writeln!(out, "{json}")?;
                }
                FormatArg::Sp => {
                    anyhow::bail!("--format sp is import-only, not an export format")
                }
            }
        }

        Cmd::Import {
            file,
            format,
            parent,
        } => {
            let file = req.cwd.join(file);
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            let fmt = format.unwrap_or_else(|| {
                if file.extension().is_some_and(|e| e == "json") {
                    FormatArg::Json
                } else {
                    FormatArg::Md
                }
            });
            let parent_id = match parent.as_deref() {
                None | Some("default") => {
                    let title = dated_wrapper_title(&file)?;
                    let task = svc.create(title, None, Status::Todo, []).await?;
                    Some(task.id)
                }
                Some("root") => Some(Id::root()),
                Some(other) => {
                    let id = Id::new(other);
                    svc.snapshot(&id).await?; // fail fast with a clear "not found" if bogus
                    Some(id)
                }
            };
            match fmt {
                FormatArg::Md => {
                    let roots = svc.import_md(&text, parent_id.as_ref()).await?;
                    write_json(out, &roots)?;
                }
                FormatArg::Json => {
                    svc.import_json(&text, parent_id.as_ref()).await?;
                    write_json(out, &serde_json::json!({"ok": true}))?;
                }
                FormatArg::Sp => {
                    let roots = svc
                        .import_superproductivity(&text, parent_id.as_ref())
                        .await?;
                    write_json(out, &roots)?;
                }
            }
        }

        Cmd::Ws { cmd } => match cmd {
            Some(WsCmd::Init { path, name, root }) => {
                let folder = match path {
                    Some(p) => req.cwd.join(p),
                    None => req.cwd.clone(),
                };
                let folder = folder
                    .canonicalize()
                    .with_context(|| format!("canonicalize {}", folder.display()))?;
                let name = name.unwrap_or_else(|| {
                    folder
                        .file_name()
                        .map_or_else(|| "workspace".into(), |n| n.to_string_lossy().into_owned())
                });
                let root_id = match root {
                    Some(id) => Id::new(id),
                    None => svc.create(name.clone(), None, Status::Todo, []).await?.id,
                };
                let ws = Workspace {
                    name,
                    path: Some(folder.to_string_lossy().into_owned()),
                };
                let task = svc.set_workspace(&root_id, Some(ws)).await?;
                write_json(out, &task)?;
            }
            None => {
                let overrides = todoapp_config::workspace_overrides();
                let root = svc.workspace_root_for(&req.cwd, &overrides).await;
                match root {
                    Some(id) => {
                        let ws = effective_workspace(svc, &id, &overrides).await;
                        write_json(out, &serde_json::json!({"root": id, "workspace": ws}))?;
                    }
                    None => anyhow::bail!(
                        "no workspace found for {} (see `tda ws init`)",
                        req.cwd.display()
                    ),
                }
            }
        },

        Cmd::Attach { id, file } => {
            let file = req.cwd.join(file);
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let title = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let task = svc
                .add_attachment_from_bytes(&Id::new(id), title, bytes, None)
                .await?;
            write_json(out, &task)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svc::{SystemClock, UlidGen, make_svc};
    use todoapp_store_turso::TursoStore;

    fn req(cmd: Cmd, cwd: PathBuf, stdin: Vec<u8>) -> Request {
        Request { cmd, cwd, stdin }
    }

    /// Parse the (assumed JSON) stdout of a reply, asserting no error occurred.
    fn json(reply: &Reply) -> serde_json::Value {
        assert_eq!(reply.err, None, "command errored");
        serde_json::from_slice(&reply.out).expect("reply.out is not JSON")
    }

    #[tokio::test]
    async fn add_then_ls_roundtrips_through_the_store() {
        let store = TursoStore::open_memory().await;
        let (clock, ids) = (SystemClock, UlidGen);
        let svc = make_svc(&store, &clock, &ids);
        let cwd = PathBuf::from("/");

        let add = run_command(
            &svc,
            &req(
                Cmd::Add {
                    title: Some("write tests".into()),
                    parent: None,
                    tags: vec![],
                    status: StatusArg::Todo,
                    batch: false,
                },
                cwd.clone(),
                vec![],
            ),
        )
        .await;
        let created = json(&add);
        assert_eq!(created["title"], "write tests");
        let id = created["id"].as_str().unwrap().to_string();

        let ls = run_command(
            &svc,
            &req(
                Cmd::Ls {
                    id: None,
                    tree: false,
                },
                cwd,
                vec![],
            ),
        )
        .await;
        let roots = json(&ls);
        assert_eq!(roots.as_array().unwrap().len(), 1);
        assert_eq!(roots[0]["id"], id);
    }

    #[tokio::test]
    async fn set_changes_status() {
        let store = TursoStore::open_memory().await;
        let (clock, ids) = (SystemClock, UlidGen);
        let svc = make_svc(&store, &clock, &ids);
        let cwd = PathBuf::from("/");

        let created = json(
            &run_command(
                &svc,
                &req(
                    Cmd::Add {
                        title: Some("t".into()),
                        parent: None,
                        tags: vec![],
                        status: StatusArg::Draft,
                        batch: false,
                    },
                    cwd.clone(),
                    vec![],
                ),
            )
            .await,
        );
        let id = created["id"].as_str().unwrap().to_string();

        let set = run_command(
            &svc,
            &req(
                Cmd::Set {
                    id,
                    title: None,
                    notes: None,
                    status: Some(StatusArg::Done),
                    due: None,
                    recurrence: None,
                },
                cwd,
                vec![],
            ),
        )
        .await;
        assert_eq!(json(&set)["status"], "done");
    }

    #[tokio::test]
    async fn add_batch_reads_the_request_stdin_not_the_process_stdin() {
        let store = TursoStore::open_memory().await;
        let (clock, ids) = (SystemClock, UlidGen);
        let svc = make_svc(&store, &clock, &ids);

        let reply = run_command(
            &svc,
            &req(
                Cmd::Add {
                    title: None,
                    parent: None,
                    tags: vec![],
                    status: StatusArg::Draft,
                    batch: true,
                },
                PathBuf::from("/"),
                b"parent\n  child\n".to_vec(),
            ),
        )
        .await;
        // Non-empty output proves the tree came from req.stdin (the process
        // stdin is empty in the test harness).
        let created = json(&reply);
        assert!(!created.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_resolves_a_relative_file_against_req_cwd() {
        let store = TursoStore::open_memory().await;
        let (clock, ids) = (SystemClock, UlidGen);
        let svc = make_svc(&store, &clock, &ids);

        let dir = tempfile::tempdir().unwrap();
        // import_md wants checkbox list items, not headings.
        std::fs::write(dir.path().join("notes.md"), "- [ ] imported task\n").unwrap();

        // cwd is the temp dir; the file arg is relative — must join against cwd
        // (a plain read against the process cwd would fail "no such file").
        let reply = run_command(
            &svc,
            &req(
                Cmd::Import {
                    file: PathBuf::from("notes.md"),
                    format: None,
                    parent: Some("root".into()),
                },
                dir.path().to_path_buf(),
                vec![],
            ),
        )
        .await;
        assert_eq!(reply.err, None, "import failed: {:?}", reply.err);

        let found = json(
            &run_command(
                &svc,
                &req(
                    Cmd::Find {
                        text: "imported".into(),
                    },
                    dir.path().to_path_buf(),
                    vec![],
                ),
            )
            .await,
        );
        assert!(
            found
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["title"] == "imported task"),
            "imported task not found in {found}"
        );
    }

    #[tokio::test]
    async fn ws_init_then_q_here_scope_to_req_cwd() {
        let store = TursoStore::open_memory().await;
        let (clock, ids) = (SystemClock, UlidGen);
        let svc = make_svc(&store, &clock, &ids);

        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();

        // Bind the temp dir as a workspace root (path defaults to req.cwd).
        run_command(
            &svc,
            &req(
                Cmd::Ws {
                    cmd: Some(WsCmd::Init {
                        path: None,
                        name: Some("tmpws".into()),
                        root: None,
                    }),
                },
                cwd.clone(),
                vec![],
            ),
        )
        .await;

        // `q --here` resolves the workspace containing req.cwd — must succeed
        // (would error "no workspace found" if it used the process cwd instead).
        let q = run_command(
            &svc,
            &req(
                Cmd::Q {
                    status: vec![],
                    assignee: None,
                    under: None,
                    tag: vec![],
                    due: None,
                    sort: vec![],
                    here: true,
                },
                cwd,
                vec![],
            ),
        )
        .await;
        assert_eq!(q.err, None, "q --here failed: {:?}", q.err);
    }
}
