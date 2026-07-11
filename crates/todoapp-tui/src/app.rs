//! Application state and event handling for the tda TUI.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use todoapp_app::{Anchor, QueryHit, Services};
use todoapp_core::{
    Assignment, Clock, Date, Due, Duration, Filter, Id, IdGenerator, Query, Status,
    TaskEntityStore, Timestamp, shortest_unique_prefixes,
};
use todoapp_store_turso::TursoStore;
use tui_input::{Input, InputRequest};
use ulid::Ulid;

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::human_duration;
use crate::keymap::{Action, Keymap};
use crate::schedule::{project_finish_date, remaining_effort};
use crate::text_edit;

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
/// is `max(due, projected finish)`, red when the projection overruns `due`;
/// `None` when there's no due date and no open estimate to project from).
#[derive(Clone)]
pub struct VisibleItem {
    pub id: Id,
    pub title: String,
    pub status: Status,
    /// Worst-case rolled-up status over the task + its descendants — only
    /// meaningful (differs from `status`) when `has_children`.
    pub agg_status: Status,
    pub depth: usize,
    pub has_children: bool,
    pub is_expanded: bool,
    pub is_blocked: bool,
    pub done: usize,
    pub total: usize,
    pub by_status: BTreeMap<Status, usize>,
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
    Detail { title: String, notes: String },
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

/// Index of the title field in `TaskEditForm::fields` — multi-line, like
/// `NOTES_FIELD` (a title can span several lines, same as what the add
/// dialog already allows when creating a task).
pub const TITLE_FIELD: usize = 0;
/// Index of the notes field in `TaskEditForm::fields` — the other field that
/// gets multi-line editing (Enter inserts a newline, Up/Down move the caret
/// across wrapped rows) instead of single-line behavior.
pub const NOTES_FIELD: usize = 1;
/// Index of the read-only id field in `TaskEditForm::fields`.
pub const ID_FIELD: usize = 6;

/// How long a status-bar toast message (yank confirmation, error, ...) stays
/// visible before falling back to the keybinding hints, absent further input.
const STATUS_MSG_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// A char-index range (into the *first line* of the current row's title)
/// being extended in select mode. Order-independent — either end may be the
/// smaller index; `y` copies the inclusive `[min, max]` range.
#[derive(Clone, Copy)]
pub struct Selection {
    pub anchor: usize,
    pub cursor: usize,
}

/// Fields that get multi-line editing (Enter inserts a newline, Up/Down move
/// the caret across wrapped rows) instead of single-line behavior.
fn is_multiline_field(i: usize) -> bool {
    i == TITLE_FIELD || i == NOTES_FIELD
}

/// The multi-field task edit dialog (title/notes/due/estimate/assignee/tags/id),
/// opened by `Action::EditTitle`. Separate from `InputMode`/`input` since
/// those are single-line; this carries one buffer per field plus which one
/// is focused.
#[derive(Clone)]
pub struct TaskEditForm {
    pub id: Id,
    pub fields: [Input; 7],
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
    pub input: Option<(InputMode, Input)>,
    /// Pending delete confirmation: (task id, has-children flag).
    pub confirm_delete: Option<(Id, bool)>,
    /// Active task edit form (title/notes/due/estimate/assignee), if open.
    pub edit_form: Option<TaskEditForm>,
    /// Active char-range selection on the current row's title (Tree/List),
    /// while in select mode (entered via `Action::Select`).
    pub selection: Option<Selection>,
    /// Transient one-line message shown in the status bar, as a toast: reset
    /// on the next keypress (see `handle_event`) and auto-expires after
    /// `STATUS_MSG_TTL` even with no further input (checked in `ui.rs`'s
    /// `render_status_bar`, not mutated here — redraw is on a timer already).
    pub status_msg: Option<String>,
    status_msg_expires_at: Option<std::time::Instant>,
    pub keymap: Keymap,
    pub config: Config,
    /// Animation state for the `wip` status spinner, advanced once per redraw.
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub clipboard: Box<dyn Clipboard>,
}

/// Build a `Services` bundle from individual field references so the borrow
/// checker can see exactly which fields are in use (field-level disjoint borrows).
pub fn make_svc<'a>(
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

        // No projection when there's nothing to project from (no due date and
        // no open estimate anywhere in the subtree).
        let eta = if agg.earliest_due.is_none() && agg.remaining == Duration::ZERO {
            None
        } else {
            let remaining = remaining_effort(agg.remaining, agg.time_spent);
            let projected =
                project_finish_date(today, remaining, config.hours_per_day, config.days_per_week);
            // Overdue/eta stay day-granularity: a rendez-vous time-of-day is
            // display-only (`VisibleItem.due`, below), never compared here.
            Some(match agg.earliest_due {
                Some(due) => (due.date.max(projected), projected > due.date),
                None => (projected, false),
            })
        };
        // jscpd:ignore-start
        // ponytail: `tags.iter().cloned().collect::<Vec<_>>().join(", ")` also
        // appears in `handle_event`'s edit-form setup below; only 2 occurrences
        // of a 1-line idiom, not worth a helper. Revisit if a 3rd shows up.
        let assignees = agg
            .assignees
            .iter()
            .map(Id::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let tags = snap.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        // jscpd:ignore-end

        items.push(VisibleItem {
            id: id.clone(),
            title: snap.title,
            status: snap.status,
            agg_status: agg.status,
            depth,
            has_children,
            is_expanded,
            is_blocked,
            done: agg.done,
            total: agg.total,
            by_status: agg.by_status,
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

/// Maps a plain text-editing key to a `tui_input` request, or `None` for keys
/// handled specially by the caller (Enter/Esc/Tab/Home/End/Up/Down — each has
/// per-dialog or per-field meaning, see `handle_input_key`/`handle_edit_form_key`).
fn text_edit_request(code: KeyCode, modifiers: KeyModifiers) -> Option<InputRequest> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Char(c) if !ctrl => Some(InputRequest::InsertChar(c)),
        KeyCode::Left if ctrl => Some(InputRequest::GoToPrevWord),
        KeyCode::Left => Some(InputRequest::GoToPrevChar),
        KeyCode::Right if ctrl => Some(InputRequest::GoToNextWord),
        KeyCode::Right => Some(InputRequest::GoToNextChar),
        KeyCode::Backspace if ctrl => Some(InputRequest::DeletePrevWord),
        KeyCode::Backspace => Some(InputRequest::DeletePrevChar),
        KeyCode::Delete if ctrl => Some(InputRequest::DeleteNextWord),
        KeyCode::Delete => Some(InputRequest::DeleteNextChar),
        _ => None,
    }
}

impl AppState {
    pub async fn new(
        store: TursoStore,
        keymap: Keymap,
        config: Config,
        clipboard: Box<dyn Clipboard>,
    ) -> anyhow::Result<Self> {
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
            confirm_delete: None,
            edit_form: None,
            selection: None,
            status_msg: None,
            status_msg_expires_at: None,
            keymap,
            config,
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            clipboard,
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
            View::Detail { .. } => 0,
        }
    }

    pub fn cursor_id(&self) -> Option<Id> {
        self.items.get(self.cursor).map(|i| i.id.clone())
    }

    /// Raw (unrendered) title of the current row, Tree/List only.
    fn current_row_title(&self) -> Option<&str> {
        match &self.view {
            View::Tree | View::Help => self.items.get(self.cursor).map(|i| i.title.as_str()),
            View::List(hits) => hits.get(self.cursor).map(|h| h.task.title.as_str()),
            View::Detail { .. } => None,
        }
    }

    /// Set the toast message shown in the status bar, starting its
    /// auto-expiry countdown (see `status_msg_display`).
    fn set_status(&mut self, msg: String) {
        self.status_msg = Some(msg);
        self.status_msg_expires_at = Some(std::time::Instant::now() + STATUS_MSG_TTL);
    }

    /// The status message to display, or `None` if it has expired (in which
    /// case the caller falls back to the default keybinding hints) — a
    /// display-time check, not a mutation, so it works from `ui.rs`'s
    /// read-only `render`.
    pub fn status_msg_display(&self) -> Option<&str> {
        match (&self.status_msg, self.status_msg_expires_at) {
            (Some(msg), Some(expires_at)) if std::time::Instant::now() < expires_at => {
                Some(msg.as_str())
            }
            _ => None,
        }
    }

    fn yank(&mut self, text: &str) {
        let msg = match self.clipboard.set_text(text.to_string()) {
            Ok(()) => format!("yanked {} chars", text.chars().count()),
            Err(e) => format!("yank: {e}"),
        };
        self.set_status(msg);
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
    pub async fn handle_event(
        &mut self,
        event: crossterm::event::Event,
        term_width: u16,
    ) -> anyhow::Result<bool> {
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
        let width = text_edit::dialog_wrap_width(term_width);
        if self.edit_form.is_some() {
            return self.handle_edit_form_key(code, modifiers, width).await;
        }
        if self.input.is_some() {
            return self.handle_input_key(code, modifiers, width).await;
        }
        if self.confirm_delete.is_some() {
            return self.handle_confirm_delete_key(code).await;
        }
        if self.selection.is_some() {
            return Ok(self.handle_select_key(code));
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
                self.open_add_sibling().await;
            }
            // Add root task (tree only)
            Action::AddRoot if in_tree => {
                self.input = Some((InputMode::AddRoot, Input::default()));
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
                            ]
                            .map(Input::from),
                            focus: 0,
                        });
                    }
                }
            }
            // Delete (tree only) — arms the confirm modal, actual delete on 'y'.
            Action::Delete if in_tree => {
                if let Some(item) = self.items.get(self.cursor) {
                    self.confirm_delete = Some((item.id.clone(), item.has_children));
                }
            }
            // View rendered title/notes (tree only)
            Action::ViewDetail if in_tree => {
                if let Some(id) = self.cursor_id() {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    if let Ok(snap) = svc.snapshot(&id).await {
                        self.view = View::Detail {
                            title: snap.title,
                            notes: snap.notes.unwrap_or_default(),
                        };
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
            // Enter char-range select mode on the current row's title (any view).
            Action::Select => {
                if self.current_row_title().is_some() {
                    self.selection = Some(Selection {
                        anchor: 0,
                        cursor: 0,
                    });
                }
            }
            // Yank (copy) the whole current row's title to the clipboard (any view).
            Action::Yank => {
                if let Some(title) = self.current_row_title() {
                    let title = title.to_string();
                    self.yank(&title);
                }
            }
            // Search (any view)
            Action::Search => {
                self.input = Some((InputMode::Search, Input::default()));
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

    /// Key handling while `selection` is active: h/l/Left/Right move+extend
    /// the selection cursor within the current row's first line; `y` yanks
    /// the selected range and exits; anything else exits select mode
    /// without performing its normal (tree/list) action, matching how
    /// `edit_form`/`input` fully gate the rest of `handle_event`'s dispatch.
    fn handle_select_key(&mut self, code: KeyCode) -> bool {
        let first_line_len = self
            .current_row_title()
            .map_or(0, |t| t.split('\n').next().unwrap_or("").chars().count());
        let Some(sel) = &mut self.selection else {
            return true;
        };
        match code {
            KeyCode::Char('h') | KeyCode::Left => {
                sel.cursor = sel.cursor.saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                sel.cursor = (sel.cursor + 1).min(first_line_len.saturating_sub(1));
            }
            KeyCode::Char('y') => {
                let sel = self.selection.take().unwrap_or(Selection {
                    anchor: 0,
                    cursor: 0,
                });
                let start = sel.anchor.min(sel.cursor);
                let end = sel.anchor.max(sel.cursor);
                if let Some(title) = self.current_row_title() {
                    let first_line = title.split('\n').next().unwrap_or("");
                    let text: String = first_line
                        .chars()
                        .skip(start)
                        .take(end + 1 - start)
                        .collect();
                    self.yank(&text);
                }
            }
            _ => self.selection = None,
        }
        true
    }

    async fn handle_input_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        width: usize,
    ) -> anyhow::Result<bool> {
        // Depth-shift the cursor task while the add dialog is open, via
        // whatever chord the user has bound to reparent_in/out (default
        // alt+right/alt+left) — same effect as pressing them in tree view,
        // just reachable mid-dialog too. Computed up front since it needs
        // an immutable read of `self.input` that can't coexist with the
        // `&mut Input` borrow taken below.
        let is_add_dialog = matches!(
            self.input.as_ref().map(|(m, _)| m),
            Some(InputMode::AddChild(_) | InputMode::AddRoot)
        );
        let depth_shift_action = if is_add_dialog {
            self.keymap
                .lookup(code, modifiers)
                .filter(|a| matches!(a, Action::ReparentIn | Action::ReparentOut))
        } else {
            None
        };
        if let Some(action) = depth_shift_action {
            match action {
                Action::ReparentIn => self.reparent_in().await?,
                Action::ReparentOut => self.reparent_out().await?,
                _ => unreachable!(),
            }
            self.open_add_sibling().await;
            return Ok(true);
        }

        let Some((_, input)) = &mut self.input else {
            return Ok(true);
        };
        match code {
            // Alt+Enter submits; plain Enter inserts a newline (the dialog is
            // multi-line/soft-wrapped) — see the keybinding note in the plan:
            // Ctrl+Enter can't be reliably distinguished from plain Enter
            // without opting into the Kitty keyboard-enhancement protocol.
            KeyCode::Enter if modifiers.contains(KeyModifiers::ALT) => {
                if let Some((mode, input)) = self.input.take() {
                    let trimmed = input.value().trim().to_string();
                    if !trimmed.is_empty() {
                        let is_add = matches!(mode, InputMode::AddChild(_) | InputMode::AddRoot);
                        self.submit_input(mode, trimmed).await?;
                        if is_add && self.config.chain_add {
                            self.open_add_sibling().await;
                        }
                    }
                }
            }
            KeyCode::Enter => {
                input.handle(InputRequest::InsertChar('\n'));
            }
            KeyCode::Esc => self.input = None,
            KeyCode::Up => text_edit::move_visual_up(input, width),
            KeyCode::Down => text_edit::move_visual_down(input, width),
            KeyCode::Home => {
                let (start, _) = text_edit::current_line_bounds(input);
                input.handle(InputRequest::SetCursor(start));
            }
            KeyCode::End => {
                let (_, end) = text_edit::current_line_bounds(input);
                input.handle(InputRequest::SetCursor(end));
            }
            KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.clipboard.get_text() {
                    Ok(text) => {
                        for c in text.chars() {
                            input.handle(InputRequest::InsertChar(c));
                        }
                    }
                    Err(e) => self.set_status(format!("paste: {e}")),
                }
            }
            _ => {
                if let Some(req) = text_edit_request(code, modifiers) {
                    input.handle(req);
                }
            }
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
        width: usize,
    ) -> anyhow::Result<bool> {
        let Some(form) = &mut self.edit_form else {
            return Ok(true);
        };
        match code {
            KeyCode::Tab => form.focus = (form.focus + 1) % form.fields.len(),
            KeyCode::BackTab => {
                form.focus = (form.focus + form.fields.len() - 1) % form.fields.len();
            }
            KeyCode::Esc => self.edit_form = None,
            // Notes: Enter inserts a newline; Alt+Enter (or Enter elsewhere) saves.
            KeyCode::Enter
                if is_multiline_field(form.focus) && !modifiers.contains(KeyModifiers::ALT) =>
            {
                form.fields[form.focus].handle(InputRequest::InsertChar('\n'));
            }
            KeyCode::Enter => {
                if let Some(form) = self.edit_form.take() {
                    self.submit_edit_form(form).await?;
                }
            }
            // Up/Down move the caret within a multi-line field's wrapped
            // rows; elsewhere they're ignored (Tab/Shift+Tab is the only
            // focus navigation, freeing Up/Down for in-field caret movement).
            KeyCode::Up if is_multiline_field(form.focus) => {
                text_edit::move_visual_up(&mut form.fields[form.focus], width);
            }
            KeyCode::Down if is_multiline_field(form.focus) => {
                text_edit::move_visual_down(&mut form.fields[form.focus], width);
            }
            KeyCode::Up | KeyCode::Down => {}
            KeyCode::Home if is_multiline_field(form.focus) => {
                let (start, _) = text_edit::current_line_bounds(&form.fields[form.focus]);
                form.fields[form.focus].handle(InputRequest::SetCursor(start));
            }
            KeyCode::End if is_multiline_field(form.focus) => {
                let (_, end) = text_edit::current_line_bounds(&form.fields[form.focus]);
                form.fields[form.focus].handle(InputRequest::SetCursor(end));
            }
            KeyCode::Home => {
                form.fields[form.focus].handle(InputRequest::GoToStart);
            }
            KeyCode::End => {
                form.fields[form.focus].handle(InputRequest::GoToEnd);
            }
            KeyCode::Char('v')
                if modifiers.contains(KeyModifiers::CONTROL) && form.focus != ID_FIELD =>
            {
                match self.clipboard.get_text() {
                    Ok(text) => {
                        for c in text.chars() {
                            form.fields[form.focus].handle(InputRequest::InsertChar(c));
                        }
                    }
                    Err(e) => self.set_status(format!("paste: {e}")),
                }
            }
            _ if form.focus == ID_FIELD => {} // read-only, ignore all other edit keys
            _ => {
                if let Some(req) = text_edit_request(code, modifiers) {
                    form.fields[form.focus].handle(req);
                }
            }
        }
        Ok(true)
    }

    async fn submit_edit_form(&mut self, form: TaskEditForm) -> anyhow::Result<()> {
        let [title, notes, due, estimate, assignee, tags, _id] =
            form.fields.clone().map(String::from);
        let title = title.trim().to_string();
        if title.is_empty() {
            self.set_status("edit: title required".to_string());
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
                    self.set_status(format!("edit: due date: {e}"));
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
                    self.set_status(format!("edit: estimate: {e}"));
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
            self.set_status(format!("edit: {e}"));
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
            self.set_status(format!("status: {e}"));
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
            Err(e) => self.set_status(format!("claim: {e}")),
        }
        Ok(())
    }

    async fn handle_confirm_delete_key(&mut self, code: KeyCode) -> anyhow::Result<bool> {
        let Some((id, has_children)) = self.confirm_delete.take() else {
            return Ok(true);
        };
        if let KeyCode::Char('y' | 'Y') = code {
            let result = {
                let svc = make_svc(&self.store, &self.clock, &self.ids);
                svc.delete_task(&id, has_children).await
            };
            match result {
                Ok(()) => {
                    self.rebuild().await;
                    self.cursor = self.cursor.min(self.item_count().saturating_sub(1));
                }
                Err(e) => self.set_status(format!("delete: {e}")),
            }
        }
        Ok(true)
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
                self.set_status(format!("reorder: {e}"));
            }
            self.rebuild().await;
            if let Some(pos) = self.items.iter().position(|i| i.id == item.id) {
                self.cursor = pos;
            }
        }
        Ok(())
    }

    /// Open the add-task dialog targeting a sibling of the cursor task (or a
    /// root task if the cursor has no parent / the list is empty). Also used
    /// to reopen/retarget the dialog after a chained submit or a depth shift.
    async fn open_add_sibling(&mut self) {
        self.input = Some(match self.cursor_id() {
            Some(id) => {
                let svc = make_svc(&self.store, &self.clock, &self.ids);
                match svc.parent_of(&id).await {
                    Some(parent_id) => (InputMode::AddChild(parent_id), Input::default()),
                    None => (InputMode::AddRoot, Input::default()),
                }
            }
            None => (InputMode::AddRoot, Input::default()),
        });
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
            self.set_status("reparent: no sibling above".to_string());
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
            Err(e) => self.set_status(format!("reparent: {e}")),
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
            None => self.set_status("reparent: already at top level".to_string()),
            Some(Ok(())) => {
                self.rebuild().await;
                if let Some(pos) = self.items.iter().position(|i| i.id == item.id) {
                    self.cursor = pos;
                }
            }
            Some(Err(e)) => self.set_status(format!("reparent: {e}")),
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn assignment(actor: &str) -> Assignment {
        Assignment {
            actor: Id::new(actor),
            claimed: false,
        }
    }

    const TERM_WIDTH: u16 = 120;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> crossterm::event::Event {
        crossterm::event::Event::Key(KeyEvent::new(code, modifiers))
    }

    fn press_char(c: char) -> crossterm::event::Event {
        press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub(crate) async fn new_app() -> AppState {
        AppState::new(
            TursoStore::open_memory().await,
            Keymap::load(None).unwrap(),
            Config::load(None).unwrap(),
            Box::new(crate::clipboard::FakeClipboard::default()),
        )
        .await
        .unwrap()
    }

    async fn type_str(app: &mut AppState, s: &str) {
        for c in s.chars() {
            app.handle_event(press_char(c), TERM_WIDTH).await.unwrap();
        }
    }

    /// End-to-end: opening the add-root dialog, moving the caret to fix a
    /// typo mid-string (not just append/backspace-last), and submitting with
    /// Alt+Enter creates a task with the corrected title.
    #[tokio::test]
    async fn add_dialog_supports_mid_string_insert_via_real_cursor() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap(); // add_sibling -> AddRoot (tree empty)
        type_str(&mut app, "Helo").await;
        app.handle_event(press(KeyCode::Left, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        type_str(&mut app, "l").await; // fix "Helo" -> "Hello" by inserting before the final 'o'
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "Hello");

        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_none());
        assert!(app.items.iter().any(|i| i.title == "Hello"));
    }

    /// Home/End jump within the current logical line; Enter inserts a
    /// newline instead of submitting (Alt+Enter submits).
    #[tokio::test]
    async fn add_dialog_is_multiline_with_home_end_per_line() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "foo bar").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        type_str(&mut app, "baz").await;
        {
            let (_, input) = app.input.as_ref().unwrap();
            assert_eq!(input.value(), "foo bar\nbaz");
        }

        app.handle_event(press(KeyCode::Home, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        {
            let (_, input) = app.input.as_ref().unwrap();
            assert_eq!(input.cursor(), 8); // start of the "baz" line, not the whole buffer
        }
        app.handle_event(press(KeyCode::End, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        {
            let (_, input) = app.input.as_ref().unwrap();
            assert_eq!(input.cursor(), 11); // end of "baz", the whole value's length
        }

        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.items.iter().any(|i| i.title == "foo bar\nbaz"));
    }

    /// Ctrl+Left/Right (word-wise jump) and Ctrl+Backspace (word-wise
    /// delete) work in the add dialog.
    #[tokio::test]
    async fn add_dialog_supports_word_wise_navigation_and_deletion() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "foo bar baz").await;
        app.handle_event(press(KeyCode::Left, KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        {
            let (_, input) = app.input.as_ref().unwrap();
            assert_eq!(input.cursor(), 8); // jumped to the start of "baz"
        }
        app.handle_event(press(KeyCode::Backspace, KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "foo baz"); // "bar " word-deleted
    }

    async fn new_app_with(keymap_toml: Option<&str>, config_toml: Option<&str>) -> AppState {
        AppState::new(
            TursoStore::open_memory().await,
            Keymap::load(keymap_toml).unwrap(),
            Config::load(config_toml).unwrap(),
            Box::new(crate::clipboard::FakeClipboard::default()),
        )
        .await
        .unwrap()
    }

    fn depth_of(app: &AppState, title: &str) -> Option<usize> {
        app.items.iter().find(|i| i.title == title).map(|i| i.depth)
    }

    /// With `behavior.chain_add = true`, Alt+Enter reopens the add dialog
    /// at the same level instead of closing it, so several tasks can be
    /// added back-to-back without pressing `a` again.
    #[tokio::test]
    async fn chain_add_keeps_dialog_open_at_same_level() {
        let mut app = new_app_with(None, Some("[behavior]\nchain_add = true\n")).await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_some(), "dialog stays open when chaining");

        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_some());
        assert_eq!(depth_of(&app, "A"), Some(0));
        assert_eq!(depth_of(&app, "B"), Some(0)); // sibling of A, not nested
    }

    /// Without chaining, Alt+Enter closes the dialog as before (no
    /// regression from the `chain_add` feature).
    #[tokio::test]
    async fn chain_add_off_by_default_closes_dialog() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_none());
    }

    /// Alt+Right/Alt+Left (`reparent_in`/`reparent_out`'s default chords) work
    /// inside the add dialog to shift the cursor task's depth, independent
    /// of `chain_add` — same as pressing them in tree view.
    #[tokio::test]
    async fn depth_shift_works_in_add_dialog_regardless_of_chain_add() {
        let mut app = new_app().await; // chain_add = false (default)
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_none()); // dialog closed, not chaining

        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(0)); // sibling of A at root

        // Reopen the dialog on B and nest it under A via alt+right.
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        app.handle_event(press(KeyCode::Right, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(1)); // now a child of A
        let (mode, _) = app.input.as_ref().unwrap();
        assert!(matches!(mode, InputMode::AddChild(_)));

        // Alt+left outdents it back to root.
        app.handle_event(press(KeyCode::Left, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(0));
    }

    /// `reparent_in`'s chord is read live from the keymap, not hardcoded —
    /// rebinding it still drives the same depth-shift-in-dialog behavior.
    #[tokio::test]
    async fn depth_shift_honors_rebound_chord() {
        let mut app = new_app_with(Some("[keybindings]\nreparent_in = [\"ctrl+j\"]\n"), None).await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();

        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        app.handle_event(press(KeyCode::Char('j'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(1)); // rebound chord nested B under A
    }

    /// The edit form's notes field is multi-line (Enter inserts a newline,
    /// Up/Down move the caret across rows); the other fields stay
    /// single-line. Saving with Alt+Enter persists the multi-line notes.
    #[tokio::test]
    async fn edit_form_notes_field_is_multiline_and_saves() {
        let (mut app, task_id) = new_app_with_task().await;

        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap(); // edit_title
        assert!(app.edit_form.is_some());
        app.handle_event(press(KeyCode::Tab, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // focus -> notes
        assert_eq!(app.edit_form.as_ref().unwrap().focus, NOTES_FIELD);

        type_str(&mut app, "line one").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // newline, not submit (focus is notes)
        type_str(&mut app, "line two").await;
        assert!(app.edit_form.is_some(), "Enter on notes must not submit");
        {
            let form = app.edit_form.as_ref().unwrap();
            assert_eq!(form.fields[NOTES_FIELD].value(), "line one\nline two");
        }

        app.handle_event(press(KeyCode::Up, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        {
            let form = app.edit_form.as_ref().unwrap();
            // moved up from end of "line two" (col 8) to the same column on "line one"
            assert_eq!(form.fields[NOTES_FIELD].cursor(), 8);
        }

        let snap = save_edit_form(&mut app, &task_id).await;
        assert_eq!(snap.notes.as_deref(), Some("line one\nline two"));
    }

    /// The title field is multi-line too (mirrors notes): a title created
    /// with embedded newlines via the add dialog can be re-edited and
    /// re-saved with those newlines intact.
    #[tokio::test]
    async fn edit_form_title_field_is_multiline_and_saves() {
        let (mut app, task_id) = new_app_with_task().await;

        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, TITLE_FIELD);

        // Replace the title with a two-line one.
        for _ in 0..4 {
            app.handle_event(press(KeyCode::Backspace, KeyModifiers::CONTROL), TERM_WIDTH)
                .await
                .unwrap();
        }
        type_str(&mut app, "Title one").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // newline, not submit (focus is title)
        type_str(&mut app, "Title two").await;
        assert!(app.edit_form.is_some(), "Enter on title must not submit");
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[TITLE_FIELD].value(),
            "Title one\nTitle two"
        );

        let snap = save_edit_form(&mut app, &task_id).await;
        assert_eq!(snap.title, "Title one\nTitle two");
    }

    /// The id field is read-only: no key (typing, backspace, ...) changes
    /// its displayed value.
    #[tokio::test]
    async fn edit_form_id_field_is_read_only() {
        let (mut app, _task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();

        tab_to_id_field(&mut app).await;
        let before = app.edit_form.as_ref().unwrap().fields[ID_FIELD]
            .value()
            .to_string();
        app.handle_event(press_char('x'), TERM_WIDTH).await.unwrap();
        app.handle_event(press(KeyCode::Backspace, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[ID_FIELD].value(),
            before
        );
    }

    /// Up/Down no longer cycle focus (that's Tab/Shift+Tab only) now that
    /// they mean "move the caret" within the notes field.
    #[tokio::test]
    async fn edit_form_up_down_do_not_change_focus_outside_notes() {
        let (mut app, _task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, 0);

        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, 0);
        app.handle_event(press(KeyCode::Up, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, 0);

        app.handle_event(press(KeyCode::Tab, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, 1);
        app.handle_event(press(KeyCode::BackTab, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.edit_form.as_ref().unwrap().focus, 0);
    }

    /// Tabs forward to the read-only id field of the open edit form.
    async fn tab_to_id_field(app: &mut AppState) {
        for _ in 0..ID_FIELD {
            app.handle_event(press(KeyCode::Tab, KeyModifiers::NONE), TERM_WIDTH)
                .await
                .unwrap();
        }
        assert_eq!(app.edit_form.as_ref().unwrap().focus, ID_FIELD);
    }

    /// Saves the open edit form (Alt+Enter) and returns the task's snapshot.
    async fn save_edit_form(app: &mut AppState, task_id: &Id) -> todoapp_app::TaskSnapshot {
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.edit_form.is_none());
        let svc = make_svc(&app.store, &app.clock, &app.ids);
        svc.snapshot(task_id).await.unwrap()
    }

    /// Creates a plain "Task", rebuilds the tree, and positions the cursor on it.
    async fn new_app_with_task() -> (AppState, Id) {
        let mut app = new_app().await;
        let task_id = {
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            svc.create("Task", None, Status::Todo, []).await.unwrap().id
        };
        app.rebuild().await;
        app.cursor = app.items.iter().position(|i| i.id == task_id).unwrap();
        (app, task_id)
    }

    async fn create_task_titled(app: &mut AppState, title: &str) {
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(app, title).await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn yank_with_no_selection_copies_whole_title() {
        let mut app = new_app().await;
        create_task_titled(&mut app, "Hello world").await;
        app.handle_event(press_char('y'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.clipboard.get_text().unwrap(), "Hello world");
    }

    #[tokio::test]
    async fn select_mode_extends_and_yanks_a_character_range() {
        let mut app = new_app().await;
        create_task_titled(&mut app, "Hello world").await;
        app.handle_event(press_char('V'), TERM_WIDTH).await.unwrap();
        assert!(app.selection.is_some());
        for _ in 0..4 {
            app.handle_event(press(KeyCode::Right, KeyModifiers::NONE), TERM_WIDTH)
                .await
                .unwrap();
        }
        app.handle_event(press_char('y'), TERM_WIDTH).await.unwrap();
        assert!(app.selection.is_none());
        assert_eq!(app.clipboard.get_text().unwrap(), "Hello");
    }

    #[tokio::test]
    async fn select_mode_blocks_row_navigation_and_exits_on_other_keys() {
        let mut app = new_app().await;
        create_task_titled(&mut app, "Hello world").await;
        create_task_titled(&mut app, "Second task").await;
        app.handle_event(press_char('V'), TERM_WIDTH).await.unwrap();
        let cursor_before = app.cursor;
        app.handle_event(press_char('j'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.cursor, cursor_before);
        assert!(app.selection.is_none());
    }

    #[tokio::test]
    async fn ctrl_v_pastes_clipboard_into_add_dialog() {
        let mut app = new_app().await;
        app.clipboard.set_text("pasted text".to_string()).unwrap();
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        app.handle_event(press(KeyCode::Char('v'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "pasted text");
    }

    #[tokio::test]
    async fn ctrl_v_paste_is_blocked_on_id_field_but_works_on_title_field() {
        let (mut app, _task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        app.clipboard.set_text("pasted".to_string()).unwrap();

        tab_to_id_field(&mut app).await;
        let before = app.edit_form.as_ref().unwrap().fields[ID_FIELD]
            .value()
            .to_string();
        app.handle_event(press(KeyCode::Char('v'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[ID_FIELD].value(),
            before
        );

        for _ in 0..ID_FIELD {
            app.handle_event(press(KeyCode::BackTab, KeyModifiers::NONE), TERM_WIDTH)
                .await
                .unwrap();
        }
        assert_eq!(app.edit_form.as_ref().unwrap().focus, TITLE_FIELD);
        app.handle_event(press(KeyCode::Char('v'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[TITLE_FIELD].value(),
            "Taskpasted"
        );
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
