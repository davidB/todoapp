//! Application state and event handling for the tda TUI.

use std::collections::{HashMap, HashSet};

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tda_app::{Anchor, QueryHit, Services};
use tda_core::{
    Assignment, Clock, Date, Due, Duration, Filter, Id, IdGenerator, Query, Status,
    TaskEntityStore, Timestamp, shortest_unique_prefixes,
};
use tda_store_turso::TursoStore;
use ulid::Ulid;

use crate::config::Config;
use crate::human_duration;
use crate::keymap::{Action, Keymap};
use crate::schedule::project_finish_date;

// ---- Clock & IdGenerator ----------------------------------------------------

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        #[allow(clippy::cast_possible_truncation)]
        Timestamp::from_millisecond(ms as i64)
    }
    fn today(&self) -> Date {
        Date(jiff::Zoned::now().date())
    }
}

pub struct UlidGen;

impl IdGenerator for UlidGen {
    fn next_id(&self) -> Id {
        Id::new(Ulid::new().to_string().to_lowercase())
    }
}

// ---- View types -------------------------------------------------------------

/// One row in the rendered tree table. The non-tree columns are aggregates
/// over the task + its descendants (`Services::aggregate`, spec-driven: eta
/// is `max(due, projected finish)`, red when the projection overruns `due`).
#[derive(Clone)]
pub struct VisibleItem {
    pub id: Id,
    pub title: String,
    pub status: Status,
    pub depth: usize,
    pub has_children: bool,
    pub is_expanded: bool,
    pub is_blocked: bool,
    pub done: usize,
    pub total: usize,
    pub due: Option<Due>,
    /// `(projected/effective eta date, true if it overruns `due`)`.
    pub eta: Option<(Date, bool)>,
    pub assignees: String,
    pub estimate: Duration,
    pub elapsed: Duration,
    pub tags: String,
}

#[derive(Clone)]
pub enum View {
    Tree,
    List(Vec<QueryHit>),
    Help,
}

#[derive(Clone)]
pub enum InputMode {
    AddChild(Id),
    AddRoot,
    Search,
}

/// Field labels for `TaskEditForm`, in `fields` order. `id` is shown for
/// reference but not written back on save (see `submit_edit_form`).
pub const EDIT_FORM_LABELS: [&str; 7] = [
    "title",
    "notes",
    "due (YYYY-MM-DD[ HH:MM])",
    "estimate (min)",
    "assignee",
    "tags",
    "id (read-only)",
];

/// The multi-field task edit dialog (title/notes/due/estimate/assignee/tags/id),
/// opened by `Action::EditTitle`. Separate from `InputMode`/`input` since
/// those are single-line; this carries one buffer per field plus which one
/// is focused.
#[derive(Clone)]
pub struct TaskEditForm {
    pub id: Id,
    pub fields: [String; 7],
    pub focus: usize,
}

// ---- AppState ---------------------------------------------------------------

pub struct AppState {
    pub store: TursoStore,
    pub clock: SystemClock,
    pub ids: UlidGen,
    /// Flat, ordered list of visible items for the tree view (rebuilt after mutations).
    pub items: Vec<VisibleItem>,
    /// Shortest unique prefix per task id (git/jj-style abbreviation),
    /// recomputed against *all* ids (not just `items`) on every `rebuild`.
    pub short_ids: HashMap<Id, String>,
    pub cursor: usize,
    pub expanded: HashSet<Id>,
    pub view: View,
    /// Active input modal: (mode, typed text).
    pub input: Option<(InputMode, String)>,
    /// Active task edit form (title/notes/due/estimate/assignee), if open.
    pub edit_form: Option<TaskEditForm>,
    /// Transient one-line message shown in the status bar.
    pub status_msg: Option<String>,
    pub keymap: Keymap,
    pub config: Config,
    /// Animation state for the `wip` status spinner, advanced once per redraw.
    pub throbber_state: throbber_widgets_tui::ThrobberState,
}

/// Build a `Services` bundle from individual field references so the borrow
/// checker can see exactly which fields are in use (field-level disjoint borrows).
fn make_svc<'a>(
    store: &'a TursoStore,
    clock: &'a SystemClock,
    ids: &'a UlidGen,
) -> Services<'a, TursoStore> {
    Services {
        store,
        links: store,
        collections: store,
        query: store,
        clock,
        ids,
        blobs: store,
    }
}

/// Rebuild the flat visible-item list by DFS over the tree.
/// Takes fields separately so the caller can mutate `items`/`cursor` afterwards.
/// ponytail: one async call per visible item for `is_blocked`/`aggregate`; fine
/// for a local tool.
async fn build_visible_items(
    store: &TursoStore,
    clock: &SystemClock,
    ids: &UlidGen,
    expanded: &HashSet<Id>,
    config: &Config,
) -> Vec<VisibleItem> {
    let svc = make_svc(store, clock, ids);
    let today = clock.today();
    let roots = svc.roots().await;
    let mut items: Vec<VisibleItem> = Vec::new();
    let mut stack: Vec<(Id, usize)> = roots.into_iter().rev().map(|id| (id, 0)).collect();

    while let Some((id, depth)) = stack.pop() {
        let Ok(snap) = svc.snapshot(&id).await else {
            continue;
        };
        let children = svc.children_of(&id).await;
        let has_children = !children.is_empty();
        let is_expanded = expanded.contains(&id);
        let is_blocked = svc.is_blocked(&id).await;
        let agg = svc.aggregate(&id).await.unwrap_or_default();

        let projected = project_finish_date(
            today,
            agg.remaining,
            config.hours_per_day,
            config.days_per_week,
        );
        // Overdue/eta stay day-granularity: a rendez-vous time-of-day is
        // display-only (`VisibleItem.due`, below), never compared here.
        let eta = Some(match agg.earliest_due {
            Some(due) => (due.date.max(projected), projected > due.date),
            None => (projected, false),
        });
        let assignees = agg
            .assignees
            .iter()
            .map(Id::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let tags = snap.tags.iter().cloned().collect::<Vec<_>>().join(", ");

        items.push(VisibleItem {
            id: id.clone(),
            title: snap.title,
            status: snap.status,
            depth,
            has_children,
            is_expanded,
            is_blocked,
            done: agg.done,
            total: agg.total,
            due: agg.earliest_due,
            eta,
            assignees,
            estimate: agg.estimate,
            elapsed: agg.time_spent,
            tags,
        });

        if is_expanded && has_children {
            for child in children.iter().rev() {
                stack.push((child.to.clone(), depth + 1));
            }
        }
    }
    items
}

/// Diff a comma-separated actor-id field against a task's current
/// `Assignment`s: `(to_assign, to_unassign)`.
fn diff_assignees(current: &[Assignment], text: &str) -> (Vec<Id>, Vec<Id>) {
    let wanted: Vec<Id> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Id::new)
        .collect();
    let to_assign = wanted
        .iter()
        .filter(|id| !current.iter().any(|a| &a.actor == *id))
        .cloned()
        .collect();
    let to_unassign = current
        .iter()
        .map(|a| &a.actor)
        .filter(|actor| !wanted.contains(actor))
        .cloned()
        .collect();
    (to_assign, to_unassign)
}

/// Diff a comma-separated tag field against a task's current tags:
/// `(to_add, to_remove)`.
fn diff_tags(
    current: &std::collections::BTreeSet<String>,
    text: &str,
) -> (Vec<String>, Vec<String>) {
    let wanted: Vec<String> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let to_add = wanted
        .iter()
        .filter(|t| !current.contains(*t))
        .cloned()
        .collect();
    let to_remove = current
        .iter()
        .filter(|t| !wanted.contains(t))
        .cloned()
        .collect();
    (to_add, to_remove)
}

impl AppState {
    pub async fn new(store: TursoStore, keymap: Keymap, config: Config) -> anyhow::Result<Self> {
        let mut app = Self {
            store,
            clock: SystemClock,
            ids: UlidGen,
            items: Vec::new(),
            short_ids: HashMap::new(),
            cursor: 0,
            expanded: HashSet::new(),
            view: View::Tree,
            input: None,
            edit_form: None,
            status_msg: None,
            keymap,
            config,
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
        };
        app.rebuild().await;
        Ok(app)
    }

    pub async fn rebuild(&mut self) {
        let new_items = build_visible_items(
            &self.store,
            &self.clock,
            &self.ids,
            &self.expanded,
            &self.config,
        )
        .await;
        self.items = new_items;
        self.short_ids = shortest_unique_prefixes(&self.store.all().await);
        if self.cursor >= self.items.len() {
            self.cursor = self.items.len().saturating_sub(1);
        }
    }

    fn item_count(&self) -> usize {
        match &self.view {
            View::Tree | View::Help => self.items.len(),
            View::List(hits) => hits.len(),
        }
    }

    pub fn cursor_id(&self) -> Option<Id> {
        self.items.get(self.cursor).map(|i| i.id.clone())
    }

    fn move_cursor(&mut self, down: bool) {
        let len = self.item_count();
        if len == 0 {
            return;
        }
        if down {
            self.cursor = (self.cursor + 1).min(len - 1);
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    /// Returns `false` to signal quit.
    #[allow(clippy::too_many_lines)]
    pub async fn handle_event(&mut self, event: crossterm::event::Event) -> anyhow::Result<bool> {
        let crossterm::event::Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event
        else {
            return Ok(true);
        };
        if kind != KeyEventKind::Press {
            return Ok(true);
        }
        // Always quit on Ctrl+C.
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(false);
        }
        if self.edit_form.is_some() {
            return self.handle_edit_form_key(code, modifiers).await;
        }
        if self.input.is_some() {
            return self.handle_input_key(code, modifiers).await;
        }
        self.status_msg = None;
        let in_tree = matches!(self.view, View::Tree);

        let Some(action) = self.keymap.lookup(code, modifiers) else {
            return Ok(true);
        };

        match action {
            Action::Quit => {
                if in_tree {
                    return Ok(false);
                }
                self.view = View::Tree;
                self.cursor = 0;
            }
            Action::ToggleHelp => {
                self.view = if matches!(self.view, View::Help) {
                    View::Tree
                } else {
                    View::Help
                };
            }
            // Navigation (any view)
            Action::MoveDown => self.move_cursor(true),
            Action::MoveUp => self.move_cursor(false),
            Action::JumpFirst => self.cursor = 0,
            Action::JumpLast => self.cursor = self.item_count().saturating_sub(1),
            // Expand (tree only)
            Action::Expand if in_tree => {
                if let Some(item) = self.items.get(self.cursor)
                    && item.has_children
                {
                    let id = item.id.clone();
                    self.expanded.insert(id);
                    self.rebuild().await;
                }
            }
            // Open search/what-next result in the tree, cursor on the selected task.
            Action::Expand => {
                let View::List(hits) = &self.view else {
                    return Ok(true);
                };
                let Some(id) = hits.get(self.cursor).map(|h| h.task.id.clone()) else {
                    return Ok(true);
                };
                let mut ancestors = Vec::new();
                {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    let mut cur = id.clone();
                    while let Some(parent_id) = svc.parent_of(&cur).await {
                        ancestors.push(parent_id.clone());
                        cur = parent_id;
                    }
                }
                self.expanded.extend(ancestors);
                self.rebuild().await;
                self.view = View::Tree;
                if let Some(pos) = self.items.iter().position(|i| i.id == id) {
                    self.cursor = pos;
                }
            }
            // Collapse / jump to parent (tree only)
            Action::Collapse if in_tree => {
                if let Some(item) = self.items.get(self.cursor).cloned() {
                    if item.is_expanded {
                        self.expanded.remove(&item.id);
                        self.rebuild().await;
                    } else if item.depth > 0 {
                        let parent_depth = item.depth - 1;
                        if let Some(pos) = self.items[..self.cursor]
                            .iter()
                            .rposition(|i| i.depth == parent_depth)
                        {
                            self.cursor = pos;
                        }
                    }
                }
            }
            // Add sibling of cursor (or root task if list is empty) — tree only.
            Action::AddSibling if in_tree => {
                self.input = Some(match self.cursor_id() {
                    Some(id) => {
                        let svc = make_svc(&self.store, &self.clock, &self.ids);
                        match svc.parent_of(&id).await {
                            Some(parent_id) => (InputMode::AddChild(parent_id), String::new()),
                            None => (InputMode::AddRoot, String::new()),
                        }
                    }
                    None => (InputMode::AddRoot, String::new()),
                });
            }
            // Add root task (tree only)
            Action::AddRoot if in_tree => {
                self.input = Some((InputMode::AddRoot, String::new()));
            }
            // Edit task (title/notes/due/estimate/assignee) — tree only
            Action::EditTitle if in_tree => {
                if let Some(id) = self.cursor_id() {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    if let Ok(snap) = svc.snapshot(&id).await {
                        let assignee = snap
                            .assignments
                            .iter()
                            .map(|a| a.actor.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let tags = snap.tags.iter().cloned().collect::<Vec<_>>().join(", ");
                        let id_display = self
                            .short_ids
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| id.to_string());
                        self.edit_form = Some(TaskEditForm {
                            id,
                            fields: [
                                snap.title,
                                snap.notes.unwrap_or_default(),
                                snap.due_date.map(|d| d.to_string()).unwrap_or_default(),
                                snap.eta_minutes
                                    .map(|e| {
                                        human_duration::format(
                                            e,
                                            self.config.hours_per_day,
                                            self.config.days_per_week,
                                        )
                                    })
                                    .unwrap_or_default(),
                                assignee,
                                tags,
                                id_display,
                            ],
                            focus: 0,
                        });
                    }
                }
            }
            // Cycle status (tree only)
            Action::CycleStatus if in_tree => {
                self.cycle_status().await?;
            }
            // Claim (tree only)
            Action::Claim if in_tree => {
                self.claim().await?;
            }
            // Reorder among siblings (tree only)
            Action::ReorderDown if in_tree => {
                self.reorder_sibling(true).await?;
            }
            Action::ReorderUp if in_tree => {
                self.reorder_sibling(false).await?;
            }
            // Reparent (tree only)
            Action::ReparentIn if in_tree => {
                self.reparent_in().await?;
            }
            Action::ReparentOut if in_tree => {
                self.reparent_out().await?;
            }
            // Search (any view)
            Action::Search => {
                self.input = Some((InputMode::Search, String::new()));
            }
            // What-next (any view)
            Action::WhatNext => {
                let hits = {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    svc.what_next().await
                };
                self.view = View::List(hits);
                self.cursor = 0;
            }
            _ => {}
        }
        Ok(true)
    }

    async fn handle_input_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<bool> {
        match code {
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some((_, ref mut text)) = self.input {
                    text.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some((_, ref mut text)) = self.input {
                    text.pop();
                }
            }
            KeyCode::Esc => {
                self.input = None;
            }
            KeyCode::Enter => {
                if let Some((mode, text)) = self.input.take() {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        self.submit_input(mode, trimmed).await?;
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    async fn submit_input(&mut self, mode: InputMode, text: String) -> anyhow::Result<()> {
        match mode {
            InputMode::AddChild(parent_id) => {
                let new_id = {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    svc.create(text, Some(&parent_id), Status::Draft, [])
                        .await
                        .context("create child")?
                        .id
                };
                self.expanded.insert(parent_id);
                self.rebuild().await;
                if let Some(pos) = self.items.iter().position(|i| i.id == new_id) {
                    self.cursor = pos;
                }
            }
            InputMode::AddRoot => {
                let new_id = {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    svc.create(text, None, Status::Draft, [])
                        .await
                        .context("create task")?
                        .id
                };
                self.rebuild().await;
                if let Some(pos) = self.items.iter().position(|i| i.id == new_id) {
                    self.cursor = pos;
                }
            }
            InputMode::Search => {
                let hits = {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    svc.evaluate(&Query {
                        filter: Filter {
                            text: Some(text),
                            ..Default::default()
                        },
                        sort: vec![],
                    })
                    .await
                };
                self.view = View::List(hits);
                self.cursor = 0;
            }
        }
        Ok(())
    }

    async fn handle_edit_form_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<bool> {
        match code {
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(form) = &mut self.edit_form {
                    form.fields[form.focus].push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(form) = &mut self.edit_form {
                    form.fields[form.focus].pop();
                }
            }
            KeyCode::Tab | KeyCode::Down => {
                if let Some(form) = &mut self.edit_form {
                    form.focus = (form.focus + 1) % form.fields.len();
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(form) = &mut self.edit_form {
                    form.focus = (form.focus + form.fields.len() - 1) % form.fields.len();
                }
            }
            KeyCode::Esc => {
                self.edit_form = None;
            }
            KeyCode::Enter => {
                if let Some(form) = self.edit_form.take() {
                    self.submit_edit_form(form).await?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    async fn submit_edit_form(&mut self, form: TaskEditForm) -> anyhow::Result<()> {
        let [title, notes, due, estimate, assignee, tags, _id] = form.fields.clone();
        let title = title.trim().to_string();
        if title.is_empty() {
            self.status_msg = Some("edit: title required".to_string());
            self.edit_form = Some(form);
            return Ok(());
        }
        let due = due.trim();
        let due = if due.is_empty() {
            None
        } else {
            match Due::parse(due) {
                Ok(d) => Some(d),
                Err(e) => {
                    self.status_msg = Some(format!("edit: due date: {e}"));
                    self.edit_form = Some(form);
                    return Ok(());
                }
            }
        };
        let estimate = estimate.trim();
        let estimate = if estimate.is_empty() {
            None
        } else {
            match human_duration::parse(
                estimate,
                self.config.hours_per_day,
                self.config.days_per_week,
            ) {
                Ok(d) => Some(d),
                Err(e) => {
                    self.status_msg = Some(format!("edit: estimate: {e}"));
                    self.edit_form = Some(form);
                    return Ok(());
                }
            }
        };

        let svc = make_svc(&self.store, &self.clock, &self.ids);
        let snap = svc.snapshot(&form.id).await;
        let (to_assign, to_unassign) = match &snap {
            Ok(snap) => diff_assignees(&snap.assignments, &assignee),
            Err(_) => (Vec::new(), Vec::new()),
        };
        let (to_add_tags, to_remove_tags) = match &snap {
            Ok(snap) => diff_tags(&snap.tags, &tags),
            Err(_) => (Vec::new(), Vec::new()),
        };

        let notes = notes.trim();
        let result = svc
            .set_title(&form.id, title)
            .await
            .and(
                svc.set_notes(&form.id, (!notes.is_empty()).then(|| notes.to_string()))
                    .await,
            )
            .and(svc.set_due(&form.id, due).await)
            .and(svc.set_estimate(&form.id, estimate).await);
        for actor in to_assign {
            let _ = svc.assign(&form.id, actor).await;
        }
        for actor in to_unassign {
            let _ = svc.unassign(&form.id, actor).await;
        }
        for tag in to_add_tags {
            let _ = svc.add_tag(&form.id, tag).await;
        }
        for tag in to_remove_tags {
            let _ = svc.remove_tag(&form.id, tag).await;
        }
        if let Err(e) = result {
            self.status_msg = Some(format!("edit: {e}"));
        }
        self.rebuild().await;
        Ok(())
    }

    async fn cycle_status(&mut self) -> anyhow::Result<()> {
        let Some(item) = self.items.get(self.cursor).cloned() else {
            return Ok(());
        };
        let order = &self.config.status_order;
        let new_status = match order.iter().position(|s| *s == item.status) {
            Some(i) => order[(i + 1) % order.len()],
            None => order[0],
        };
        let result = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            svc.set_status(&item.id, new_status).await
        };
        if let Err(e) = result {
            self.status_msg = Some(format!("status: {e}"));
        }
        self.rebuild().await;
        Ok(())
    }

    async fn claim(&mut self) -> anyhow::Result<()> {
        let Some(item) = self.items.get(self.cursor).cloned() else {
            return Ok(());
        };
        // ponytail: single-user TUI — fixed actor "me"; no auth in v1 (spec §2/§13 Q5)
        let result = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            svc.claim(&item.id, Id::new("me")).await
        };
        match result {
            Ok(_) => self.rebuild().await,
            Err(e) => self.status_msg = Some(format!("claim: {e}")),
        }
        Ok(())
    }

    async fn reorder_sibling(&mut self, down: bool) -> anyhow::Result<()> {
        let Some(item) = self.items.get(self.cursor).cloned() else {
            return Ok(());
        };
        let anchor = if down {
            let start = (self.cursor + 1).min(self.items.len());
            self.items[start..]
                .iter()
                .find(|i| i.depth == item.depth)
                .map(|i| Anchor::After(i.id.clone()))
        } else {
            self.items[..self.cursor]
                .iter()
                .rfind(|i| i.depth == item.depth)
                .map(|i| Anchor::Before(i.id.clone()))
        };
        if let Some(anchor) = anchor {
            let result = {
                let svc = make_svc(&self.store, &self.clock, &self.ids);
                svc.reorder(&item.id, anchor).await
            };
            if let Err(e) = result {
                self.status_msg = Some(format!("reorder: {e}"));
            }
            self.rebuild().await;
            if let Some(pos) = self.items.iter().position(|i| i.id == item.id) {
                self.cursor = pos;
            }
        }
        Ok(())
    }

    /// Reparent the cursor task under the sibling immediately above it (indent).
    async fn reparent_in(&mut self) -> anyhow::Result<()> {
        let Some(item) = self.items.get(self.cursor).cloned() else {
            return Ok(());
        };
        let Some(prev_sibling) = self.items[..self.cursor]
            .iter()
            .rfind(|i| i.depth == item.depth)
            .cloned()
        else {
            self.status_msg = Some("reparent: no sibling above".to_string());
            return Ok(());
        };
        let result = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            svc.move_task(&item.id, &prev_sibling.id, None).await
        };
        match result {
            Ok(()) => {
                self.expanded.insert(prev_sibling.id);
                self.rebuild().await;
                if let Some(pos) = self.items.iter().position(|i| i.id == item.id) {
                    self.cursor = pos;
                }
            }
            Err(e) => self.status_msg = Some(format!("reparent: {e}")),
        }
        Ok(())
    }

    /// Move the cursor task to its parent's level, right after the former
    /// parent (outdent).
    async fn reparent_out(&mut self) -> anyhow::Result<()> {
        let Some(item) = self.items.get(self.cursor).cloned() else {
            return Ok(());
        };
        let outcome = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            match svc.parent_of(&item.id).await {
                None => None,
                Some(parent_id) => {
                    let grandparent = svc.parent_of(&parent_id).await.unwrap_or_else(Id::root);
                    Some(
                        svc.move_task(&item.id, &grandparent, Some(Anchor::After(parent_id)))
                            .await,
                    )
                }
            }
        };
        match outcome {
            None => self.status_msg = Some("reparent: already at top level".to_string()),
            Some(Ok(())) => {
                self.rebuild().await;
                if let Some(pos) = self.items.iter().position(|i| i.id == item.id) {
                    self.cursor = pos;
                }
            }
            Some(Err(e)) => self.status_msg = Some(format!("reparent: {e}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(actor: &str) -> Assignment {
        Assignment {
            actor: Id::new(actor),
            claimed: false,
        }
    }

    #[test]
    fn diff_assignees_adds_new_actor() {
        let current = [assignment("me")];
        let (assign, unassign) = diff_assignees(&current, "me, alice");
        assert_eq!(assign, vec![Id::new("alice")]);
        assert!(unassign.is_empty());
    }

    #[test]
    fn diff_assignees_removes_dropped_actor() {
        let current = [assignment("me"), assignment("alice")];
        let (assign, unassign) = diff_assignees(&current, "me");
        assert!(assign.is_empty());
        assert_eq!(unassign, vec![Id::new("alice")]);
    }
}
