//! Import tasks from a Super Productivity JSON backup export. Only fields
//! that map onto an existing tda capability are read; the rest of the export
//! (boards, planner, reminders, standalone notes, taskRepeatCfg templates,
//! project theming) is ignored. See tda-spec.md FR-27..FR-31 for the
//! capabilities this relies on.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Deserialize;
use todoapp_core::{
    Attachment, AttachmentKind, ComponentStore, Date, Due, Duration, Id, IssueRef, Status,
    TaskEntityStore,
};

use crate::service::{Error, Services, TaskSnapshot};

#[derive(Deserialize)]
struct SpExport {
    data: SpData,
}

#[derive(Deserialize)]
struct SpData {
    task: SpEntityMap<SpTask>,
    project: SpEntityMap<SpProject>,
    tag: SpEntityMap<SpTag>,
    #[serde(rename = "archiveYoung", default)]
    archive_young: Option<SpArchive>,
    #[serde(rename = "archiveOld", default)]
    archive_old: Option<SpArchive>,
}

#[derive(Deserialize)]
struct SpEntityMap<T> {
    entities: HashMap<String, T>,
}

#[derive(Deserialize)]
struct SpArchive {
    task: SpEntityMap<SpTask>,
}

#[derive(Deserialize)]
struct SpProject {
    title: String,
}

#[derive(Deserialize)]
struct SpTag {
    title: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SpTask {
    #[serde(default)]
    parent_id: Option<String>,
    title: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    is_done: bool,
    #[serde(default)]
    tag_ids: Vec<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    due_day: Option<String>,
    #[serde(default)]
    due_with_time: Option<i64>,
    #[serde(default)]
    time_estimate: i64,
    #[serde(default)]
    time_spent: i64,
    #[serde(default)]
    time_spent_on_day: HashMap<String, i64>,
    #[serde(default)]
    issue_type: Option<String>,
    #[serde(default)]
    issue_id: Option<String>,
    #[serde(default)]
    attachments: Vec<SpAttachment>,
}

#[derive(Deserialize)]
struct SpAttachment {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

/// ms → whole minutes, saturating (Duration is minute-precision).
fn minutes(ms: i64) -> u32 {
    u32::try_from(ms / 60_000).unwrap_or(0)
}

/// SP's `dueWithTime` is an epoch-ms instant; format it through the local
/// wall-clock so the due time-of-day matches what the user saw in SP.
fn ms_to_due(ms: i64) -> Option<Due> {
    let ts = jiff::Timestamp::from_millisecond(ms).ok()?;
    let dt = ts.to_zoned(jiff::tz::TimeZone::system()).datetime();
    Due::parse(&format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute()
    ))
    .ok()
}

impl<'a, St: ComponentStore + TaskEntityStore> Services<'a, St> {
    /// Import a Super Productivity JSON backup. Each SP project's root task
    /// (and any orphan task with neither `parentId` nor `projectId`) attaches
    /// under `parent`, or the virtual root if `None`. Returns the created
    /// project root tasks (one per SP project, per user decision — not a tag).
    pub async fn import_superproductivity(
        &self,
        json: &str,
        parent: Option<&Id>,
    ) -> Result<Vec<TaskSnapshot>, Error> {
        let export: SpExport =
            serde_json::from_str(json).map_err(|e| Error::Import(e.to_string()))?;
        let data = export.data;

        let tag_titles: HashMap<String, String> = data
            .tag
            .entities
            .into_iter()
            .map(|(id, t)| (id, t.title))
            .collect();

        // One top-level task per SP project; every task in that project
        // attaches under it (§B.2 decision: parent task, not a tag).
        let mut project_tda: HashMap<String, Id> = HashMap::new();
        let mut roots = Vec::new();
        for (pid, project) in data.project.entities {
            let root = self.create(project.title, parent, Status::Todo, []).await?;
            project_tda.insert(pid, root.id.clone());
            roots.push(root);
        }

        // Flatten active + archived tasks into one map (archived ones marked).
        let mut all_tasks: HashMap<String, (SpTask, bool)> = HashMap::new();
        for (id, t) in data.task.entities {
            all_tasks.insert(id, (t, false));
        }
        for archive in [data.archive_young, data.archive_old].into_iter().flatten() {
            for (id, t) in archive.task.entities {
                all_tasks.insert(id, (t, true));
            }
        }

        // Pass 1: write every task's components, minting a fresh tda `Id`.
        let now = self.clock.now();
        let mut sp_to_tda: HashMap<String, Id> = HashMap::new();
        for sp_id in all_tasks.keys() {
            sp_to_tda.insert(sp_id.clone(), self.ids.next_id());
        }
        for (sp_id, (t, archived)) in &all_tasks {
            let todoapp_id = sp_to_tda[sp_id].clone();

            let tags: BTreeSet<String> = t
                .tag_ids
                .iter()
                .filter_map(|tid| tag_titles.get(tid).cloned())
                .collect();

            let mut notes = t.notes.clone();
            if let Some(issue_id) = &t.issue_id {
                let provider = t.issue_type.clone().unwrap_or_default();
                let line = format!("\n\n_Imported from Super Productivity: {provider}#{issue_id}_");
                notes = Some(notes.unwrap_or_default() + &line);
            }

            let due_date = t
                .due_with_time
                .and_then(ms_to_due)
                .or_else(|| t.due_day.as_deref().and_then(|d| Due::parse(d).ok()));

            let issue_ref = t.issue_id.as_ref().map(|id| IssueRef {
                provider: t.issue_type.clone().unwrap_or_default(),
                id: id.clone(),
                url: None,
            });

            let time_log: BTreeMap<Date, Duration> = t
                .time_spent_on_day
                .iter()
                .filter_map(|(d, ms)| {
                    Date::parse(d)
                        .ok()
                        .map(|date| (date, Duration::from_minutes(minutes(*ms))))
                })
                .collect();

            let mut attachments = Vec::new();
            for sp_att in &t.attachments {
                attachments.push(self.to_attachment(sp_att).await);
            }

            let snapshot = TaskSnapshot {
                id: todoapp_id,
                title: t.title.clone(),
                status: if t.is_done || *archived {
                    Status::Done
                } else {
                    Status::Todo
                },
                notes,
                due_date,
                eta_minutes: (t.time_estimate > 0)
                    .then(|| Duration::from_minutes(minutes(t.time_estimate))),
                time_spent_minutes: Duration::from_minutes(minutes(t.time_spent)),
                tags,
                assignments: vec![],
                recurrence: None,
                issue_ref,
                workspace: None,
                time_log,
                archived: *archived,
                attachments,
                created_at: now,
                updated_at: now,
            };
            self.write_snapshot(&snapshot).await;
        }

        // Pass 2: attach each task under its resolved parent — its own
        // `parentId` if set, else its project's root task, else the tree root.
        for (sp_id, (t, _)) in &all_tasks {
            let todoapp_id = &sp_to_tda[sp_id];
            let parent = t
                .parent_id
                .as_ref()
                .and_then(|pid| sp_to_tda.get(pid))
                .or_else(|| t.project_id.as_ref().and_then(|pid| project_tda.get(pid)))
                .cloned()
                .unwrap_or_else(|| parent.cloned().unwrap_or_else(Id::root));
            self.attach(todoapp_id, &parent, None).await?;
        }

        Ok(roots)
    }

    /// Map one SP attachment reference; for `FILE`/`IMG`, opportunistically
    /// read the referenced path's bytes if it still resolves on this machine
    /// (SP's export JSON never embeds the bytes itself) — best-effort, never
    /// fails the import if the file is missing.
    async fn to_attachment(&self, sp: &SpAttachment) -> Attachment {
        let kind = match sp.kind.as_str() {
            "FILE" => AttachmentKind::File,
            "IMG" => AttachmentKind::Image,
            _ => AttachmentKind::Link,
        };
        let blob = if kind == AttachmentKind::Link {
            None
        } else {
            match &sp.path {
                Some(path) => match std::fs::read(path) {
                    Ok(bytes) => Some(self.blobs.put(bytes).await),
                    Err(_) => None,
                },
                None => None,
            }
        };
        Attachment {
            id: self.ids.next_id(),
            kind,
            title: sp.title.clone().unwrap_or_default(),
            url: sp.path.clone(),
            blob,
            mime: None,
        }
    }
}
