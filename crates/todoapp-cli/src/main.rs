//! tda CLI (spec §9 / M3): JSON output for agents and scripts. TUI is for humans.

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use todoapp_app::{Anchor, Services, TaskSnapshot};
use todoapp_core::{
    ComponentStore, Dir, Due, DueFilter, Filter, Id, Query, SortField, SortKey, Status,
    TaskEntityStore,
};
use todoapp_tui::{SystemClock, UlidGen, make_svc};

// ---- Output helpers ----------------------------------------------------------

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

fn print_json(v: &impl Serialize) -> anyhow::Result<()> {
    writeln!(io::stdout(), "{}", serde_json::to_string_pretty(v)?)?;
    Ok(())
}

/// One row in a flat DFS tree listing.
#[derive(Serialize)]
struct TreeLine {
    depth: usize,
    #[serde(flatten)]
    task: TaskSnapshot,
}

// ---- CLI arg types ----------------------------------------------------------

#[derive(ValueEnum, Clone, Copy)]
enum StatusArg {
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

#[derive(ValueEnum, Clone, Copy)]
enum DuePeriodArg {
    Today,
    Overdue,
}

#[derive(ValueEnum, Clone, Copy)]
enum SortArg {
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

#[derive(ValueEnum, Clone, Copy)]
enum FormatArg {
    Md,
    Json,
    /// Super Productivity JSON backup — import-only, must be requested
    /// explicitly (`--format sp`); auto-detection can't tell it apart from
    /// tda's own `--format json` by extension alone.
    #[value(alias = "superproductivity")]
    Sp,
}

// ---- Clap structs -----------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "tda",
    about = "Task and dependency manager — JSON output for agents/scripts",
    after_help = "Config: ~/.config/tda/tui.toml. Data: the OS data dir (e.g. ~/.local/share/tda/tda.db on Linux)."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
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
        /// Due date as YYYY-MM-DD, or "none" to clear.
        #[arg(long)]
        due: Option<String>,
    },
    /// Add one or more tags to a task.
    Tag { id: String, tags: Vec<String> },
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
    },
    /// What to work on next (status:todo, by priority).
    Next {
        #[arg(long = "as")]
        assignee: Option<String>,
        #[arg(long)]
        under: Option<String>,
        #[arg(long)]
        tag: Option<String>,
    },
    /// Tasks due today or overdue.
    Due { period: DuePeriodArg },
    /// Export a task subtree (default: all roots).
    Export {
        id: Option<String>,
        #[arg(long, default_value = "md")]
        format: FormatArg,
    },
    /// Import tasks from a file.
    Import {
        file: PathBuf,
        #[arg(long)]
        format: Option<FormatArg>,
    },
    /// Attach a file's contents to a task.
    Attach { id: String, file: PathBuf },
}

// ---- main -------------------------------------------------------------------

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Cmd::Tui = cli.cmd {
        return todoapp_tui::run().await;
    }

    let store = todoapp_tui::open_store().await?;
    let clock = SystemClock;
    let ids = UlidGen;
    let svc = make_svc(&store, &clock, &ids);

    match cli.cmd {
        Cmd::Tui => unreachable!("handled above"),
        Cmd::Add {
            title,
            parent,
            tags,
            status,
            batch,
        } => {
            let parent_id = parent.as_deref().map(Id::new);
            if batch {
                let text = read_stdin()?;
                let created = svc.batch_create(&text).await?;
                print_json(&created)?;
            } else {
                let title = title.context("title required unless --batch")?;
                let task = svc
                    .create(title, parent_id.as_ref(), status.into(), tags)
                    .await?;
                print_json(&task)?;
            }
        }

        Cmd::Ls { id, tree } => {
            if tree {
                let root_ids = children_of_or_roots(&svc, id).await;
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
                print_json(&lines)?;
            } else {
                let ids = children_of_or_roots(&svc, id).await;
                let mut tasks: Vec<TaskSnapshot> = Vec::new();
                for id in ids {
                    if let Ok(t) = svc.snapshot(&id).await {
                        tasks.push(t);
                    }
                }
                print_json(&tasks)?;
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
            print_json(&serde_json::json!({"ok": true}))?;
        }

        Cmd::Rm { id, recursive } => {
            svc.delete_task(&Id::new(id), recursive).await?;
            print_json(&serde_json::json!({"ok": true}))?;
        }

        Cmd::Link { from, to, kind } => {
            if kind != "blocks" {
                anyhow::bail!("only --kind blocks is supported");
            }
            svc.block(&Id::new(from), &Id::new(to)).await?;
            print_json(&serde_json::json!({"ok": true}))?;
        }

        Cmd::Assign { id, actor } => {
            let task = svc.assign(&Id::new(id), Id::new(actor)).await?;
            print_json(&task)?;
        }

        Cmd::Claim { id, actor } => {
            let task = svc.claim(&Id::new(id), Id::new(actor)).await?;
            print_json(&task)?;
        }

        Cmd::Set {
            id,
            title,
            notes,
            status,
            due,
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
                        Due::parse(&d)
                            .map_err(|e| anyhow::anyhow!(e))
                            .context("parse --due as YYYY-MM-DD or \"YYYY-MM-DD HH:MM\"")?,
                    )
                };
                task = svc.set_due(&id, val).await?;
            }
            print_json(&task)?;
        }

        Cmd::Tag { id, tags } => {
            let id = Id::new(id);
            let mut task = svc.snapshot(&id).await?;
            for tag in tags {
                task = svc.add_tag(&id, tag).await?;
            }
            print_json(&task)?;
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
            print_json(&hits.iter().map(|h| &h.task).collect::<Vec<_>>())?;
        }

        Cmd::Q {
            status,
            assignee,
            under,
            tag,
            due,
            sort,
        } => {
            let due_filter = due.map(|d| match d {
                DuePeriodArg::Today => DueFilter::Today,
                DuePeriodArg::Overdue => DueFilter::Overdue,
            });
            let hits = svc
                .evaluate(&Query {
                    filter: Filter {
                        status: status.into_iter().map(Status::from).collect(),
                        assignee: assignee.map(Id::new),
                        within: under.map(Id::new),
                        tags: tag,
                        due: due_filter,
                        ..Default::default()
                    },
                    sort: sort.into_iter().map(SortKey::from).collect(),
                })
                .await;
            print_json(&hits)?;
        }

        Cmd::Next {
            assignee,
            under,
            tag,
        } => {
            let hits = svc
                .what_next_for(assignee.map(Id::new), under.map(Id::new), tag)
                .await;
            print_json(&hits)?;
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
            print_json(&hits.iter().map(|h| &h.task).collect::<Vec<_>>())?;
        }

        Cmd::Export { id, format } => {
            let root = id.map(Id::new);
            match format {
                FormatArg::Md => {
                    let md = if let Some(id) = &root {
                        svc.export_md(id).await?
                    } else {
                        let mut out = String::new();
                        for root_id in svc.roots().await {
                            out.push_str(&svc.export_md(&root_id).await?);
                        }
                        out
                    };
                    write!(io::stdout(), "{md}")?;
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
                    writeln!(io::stdout(), "{json}")?;
                }
                FormatArg::Sp => {
                    anyhow::bail!("--format sp is import-only, not an export format")
                }
            }
        }

        Cmd::Import { file, format } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            let fmt = format.unwrap_or_else(|| {
                if file.extension().is_some_and(|e| e == "json") {
                    FormatArg::Json
                } else {
                    FormatArg::Md
                }
            });
            match fmt {
                FormatArg::Md => {
                    let roots = svc.import_md(&text).await?;
                    print_json(&roots)?;
                }
                FormatArg::Json => {
                    svc.import_json(&text).await?;
                    print_json(&serde_json::json!({"ok": true}))?;
                }
                FormatArg::Sp => {
                    let roots = svc.import_superproductivity(&text).await?;
                    print_json(&roots)?;
                }
            }
        }

        Cmd::Attach { id, file } => {
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let title = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let task = svc
                .add_attachment_from_bytes(&Id::new(id), title, bytes, None)
                .await?;
            print_json(&task)?;
        }
    }

    Ok(())
}

fn read_stdin() -> anyhow::Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf).context("read stdin")?;
    Ok(buf)
}
