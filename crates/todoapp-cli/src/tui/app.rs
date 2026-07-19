//! Application state and event handling for the tda TUI.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use todoapp_app::{Anchor, QueryHit, TaskSnapshot};
use todoapp_core::{
    Assignment, Clock, Date, Due, DueSpec, Duration, Filter, Id, LinkKind, Query, Status,
    TaskEntityStore, Workspace, shortest_unique_prefixes,
};
use todoapp_store_turso::TursoStore;
use tui_input::{Input, InputRequest};

use crate::svc::{SystemClock, UlidGen, make_svc};
use crate::tui::clipboard::Clipboard;
use crate::tui::config::Config;
use crate::tui::human_duration;
use crate::tui::keymap::{Action, Keymap};
use crate::tui::schedule::{project_finish_date, remaining_effort};
use crate::tui::text_edit;

/// Does this Enter chord submit the input dialog (vs. insert a newline)?
/// `submit_on_enter` on: plain Enter (no Shift) submits. Off: Alt+Enter submits.
fn is_submit_chord(modifiers: KeyModifiers, submit_on_enter: bool) -> bool {
    if submit_on_enter {
        !modifiers.contains(KeyModifiers::SHIFT)
    } else {
        modifiers.contains(KeyModifiers::ALT)
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
    Help,
}

/// Cached data for the details pane (`detail_shown` toggles visibility). Fetched
/// async on each handled keystroke / rebuild so it stays in sync with the
/// selection; rendered read-only in `ui::render_detail`.
pub struct DetailPane {
    /// Task this cache is for — `scroll` resets when the selection moves to a
    /// different task.
    pub id: Id,
    pub snap: TaskSnapshot,
    pub blocked: bool,
    /// Ancestor titles (root → parent).
    pub breadcrumb: Vec<String>,
    /// Titles of not-yet-done tasks blocking this one.
    pub blockers: Vec<String>,
    /// Inherited workspace name (from an ancestor), shown when the task has no
    /// workspace of its own.
    pub inherited_ws: Option<String>,
    /// Notes scroll offset (rows), driven by `DetailScroll{Up,Down}`.
    pub scroll: u16,
}

#[derive(Clone)]
pub enum InputMode {
    AddChild(Id),
    AddRoot,
    Search,
    /// Quick-assign prompt targeting one-or-more tasks (the marked set, or the
    /// cursor task when nothing is marked). Actors typed comma-separated.
    Assign(Vec<Id>),
}

/// Field labels for `TaskEditForm`, in `fields` order. `id` is shown for
/// reference but not written back on save (see `submit_edit_form`).
pub const EDIT_FORM_LABELS: [&str; 8] = [
    "title",
    "notes",
    "due (YYYY-MM-DD[ HH:MM])",
    "estimate (min)",
    "assignee",
    "tags",
    "workspace",
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
/// Index of the read-only workspace display field in `TaskEditForm::fields`
/// (own name, else dim `(inherited: X)` / `(none)`) — Enter opens the picker.
pub const WORKSPACE_FIELD: usize = 6;
/// Index of the read-only id field in `TaskEditForm::fields`.
pub const ID_FIELD: usize = 7;

/// How long a status-bar toast message (yank confirmation, error, ...) stays
/// visible before falling back to the keybinding hints, absent further input.
const STATUS_MSG_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// Rows the details pane scrolls per `DetailScroll{Up,Down}` keypress.
const DETAIL_SCROLL_STEP: u16 = 5;

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

/// The workspace field's display text when the task carries no `Workspace`
/// of its own: the nearest ancestor's name, or `(none)`.
fn workspace_placeholder(inherited: Option<&str>) -> String {
    match inherited {
        Some(name) => format!("(inherited: {name})"),
        None => "(none)".to_string(),
    }
}

/// The multi-field task edit dialog (title/notes/due/estimate/assignee/tags/id),
/// opened by `Action::EditTitle`. Separate from `InputMode`/`input` since
/// those are single-line; this carries one buffer per field plus which one
/// is focused.
#[derive(Clone)]
pub struct TaskEditForm {
    pub id: Id,
    pub fields: [Input; 8],
    pub focus: usize,
    /// Nearest ancestor's workspace name, for the `(inherited: X)` display
    /// when the task carries none of its own. `None` = no ancestor workspace.
    pub inherited: Option<String>,
    /// Staged workspace change: `None` = untouched (own unchanged),
    /// `Some(None)` = unassign, `Some(Some(w))` = assign `w`. Applied on save.
    #[allow(clippy::option_option)]
    pub workspace: Option<Option<Workspace>>,
}

/// One entry in the workspace-picker popup, with the action Enter performs.
pub struct WsPickerItem {
    pub label: String,
    pub action: WsPickerAction,
}

pub enum WsPickerAction {
    Assign(Workspace),
    Unassign,
    New,
}

/// The workspace-assignment popup opened from the edit form's workspace field.
pub struct WsPicker {
    pub items: Vec<WsPickerItem>,
    pub selected: usize,
}

/// One row of the workspace editor dialog's `name | default path | override` table.
pub struct WsRow {
    pub name: String,
    pub db_path: Option<String>,
    pub override_: Option<String>,
    pub is_new: bool,
}

/// The workspace management dialog (rename / change default path / set a
/// per-machine override, or define a brand-new workspace).
pub struct WsEditor {
    pub rows: Vec<WsRow>,
    /// `(row, col)`; col 0 = name, 1 = default path, 2 = override.
    pub cursor: (usize, usize),
    /// The cell currently being typed into, if any.
    pub editing: Option<Input>,
    /// Opened from the picker's "(new…)" entry: closing with a named new row
    /// stages that workspace onto the edit form.
    pub assign: bool,
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
    /// Batch-selection set (spec §multi-select): marking a task marks its whole
    /// subtree. Empty = ordinary single-cursor behaviour. Field ops (assign,
    /// status, claim) target every marked id; structural ops (move, delete)
    /// target only the marked *roots* so subtrees move/delete intact.
    pub marked: HashSet<Id>,
    pub view: View,
    /// Active input modal: (mode, typed text).
    pub input: Option<(InputMode, Input)>,
    /// Draft kept from a cancelled add dialog, used to seed the next one.
    /// Consumed (cleared) on a successful add submit. In-memory only.
    pub scratchpad: String,
    /// Pending delete confirmation: the marked root ids to delete (each with
    /// `recursive=true`). A single-cursor delete is just a one-element list.
    pub confirm_delete: Option<Vec<Id>>,
    /// Active task edit form (title/notes/due/estimate/assignee), if open.
    pub edit_form: Option<TaskEditForm>,
    /// Workspace-assignment popup, opened from the edit form's workspace field.
    pub ws_picker: Option<WsPicker>,
    /// Workspace management dialog, opened from the picker's "(new…)" entry.
    pub ws_editor: Option<WsEditor>,
    /// Active char-range selection on the current row's title (Tree/List),
    /// while in select mode (entered via `Action::Select`).
    pub selection: Option<Selection>,
    /// Whether the (non-modal) details pane is toggled on (`Action::ViewDetail`).
    pub detail_shown: bool,
    /// Cached details for the current selection; `None` when the pane is shown
    /// but nothing is selected. Refreshed by `refresh_detail`.
    pub detail: Option<DetailPane>,
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

/// Parse a comma-separated actor-id field (e.g. typed into the quick-assign
/// prompt or the edit form's assignee field) into ids, dropping blanks.
fn parse_actor_list(text: &str) -> Vec<Id> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Id::new)
        .collect()
}

/// Diff a comma-separated actor-id field against a task's current
/// `Assignment`s: `(to_assign, to_unassign)`.
fn diff_assignees(current: &[Assignment], text: &str) -> (Vec<Id>, Vec<Id>) {
    let wanted: Vec<Id> = parse_actor_list(text);
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
            marked: HashSet::new(),
            view: View::Tree,
            input: None,
            scratchpad: String::new(),
            confirm_delete: None,
            edit_form: None,
            ws_picker: None,
            ws_editor: None,
            selection: None,
            detail_shown: false,
            detail: None,
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

    /// Re-apply a persisted session (tree expansion + cursor), then rebuild so
    /// the restored expansion takes effect and the cursor lands on its saved
    /// task if it still exists. See [`crate::tui::state`].
    pub async fn restore(&mut self, state: crate::tui::state::UiState) {
        self.expanded = state.expanded;
        self.detail_shown = state.detail_shown;
        self.rebuild().await;
        if let Some(id) = state.cursor
            && let Some(pos) = self.items.iter().position(|i| i.id == id)
        {
            self.cursor = pos;
        }
        self.refresh_detail().await;
    }

    /// Snapshot the durable UI state for [`crate::tui::state::save`].
    pub fn ui_state(&self) -> crate::tui::state::UiState {
        crate::tui::state::UiState {
            expanded: self.expanded.clone(),
            cursor: self.cursor_id(),
            detail_shown: self.detail_shown,
        }
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
        let all_ids = self.store.all().await;
        self.short_ids = shortest_unique_prefixes(&all_ids);
        // Drop marks for tasks that no longer exist (deleted here or externally
        // via the socket) so batch targets never point at ghosts.
        if !self.marked.is_empty() {
            let live: HashSet<&Id> = all_ids.iter().collect();
            self.marked.retain(|id| live.contains(id));
        }
        if self.cursor >= self.items.len() {
            self.cursor = self.items.len().saturating_sub(1);
        }
        self.refresh_detail().await;
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

    /// Full batch target for *field* ops (assign / status / claim): every marked
    /// id — parent and all descendants — in tree order, so "assign all children
    /// in one shot" works. Falls back to the cursor task when nothing is marked.
    fn marked_ids(&self) -> Vec<Id> {
        if self.marked.is_empty() {
            return self.cursor_id().into_iter().collect();
        }
        // Tree order where visible; append any marked-but-collapsed ids so none
        // are dropped just because their subtree isn't expanded.
        let mut ids: Vec<Id> = self
            .items
            .iter()
            .filter(|i| self.marked.contains(&i.id))
            .map(|i| i.id.clone())
            .collect();
        let seen: HashSet<&Id> = ids.iter().collect();
        let missing: Vec<Id> = self
            .marked
            .iter()
            .filter(|id| !seen.contains(id))
            .cloned()
            .collect();
        ids.extend(missing);
        ids
    }

    /// Batch target for *structural* ops (move / delete): only the marked roots —
    /// marked ids whose parent is not itself marked — so whole subtrees move or
    /// delete intact. Falls back to the cursor task when nothing is marked.
    async fn marked_roots(&self) -> Vec<Id> {
        if self.marked.is_empty() {
            return self.cursor_id().into_iter().collect();
        }
        let svc = make_svc(&self.store, &self.clock, &self.ids);
        let mut roots = Vec::new();
        for id in self.marked_ids() {
            let parent_marked = match svc.parent_of(&id).await {
                Some(p) => self.marked.contains(&p),
                None => false,
            };
            if !parent_marked {
                roots.push(id);
            }
        }
        roots
    }

    /// Id of the current selection, for Tree and List views (the details pane
    /// follows this). Tree/Help use the flat item list; List uses its hits.
    pub fn selected_id(&self) -> Option<Id> {
        match &self.view {
            View::Tree | View::Help => self.items.get(self.cursor).map(|i| i.id.clone()),
            View::List(hits) => hits.get(self.cursor).map(|h| h.task.id.clone()),
        }
    }

    /// Repopulate `self.detail` for the current selection when the pane is
    /// shown. Cheap enough to run per handled keystroke / rebuild; preserves the
    /// notes scroll offset only while the selection stays on the same task.
    pub async fn refresh_detail(&mut self) {
        if !self.detail_shown {
            self.detail = None;
            return;
        }
        let Some(id) = self.selected_id() else {
            self.detail = None;
            return;
        };
        let prev_scroll = match &self.detail {
            Some(d) if d.id == id => d.scroll,
            _ => 0,
        };
        let pane = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            let Ok(snap) = svc.snapshot(&id).await else {
                self.detail = None;
                return;
            };
            let blocked = svc.is_blocked(&id).await;
            let breadcrumb = svc.breadcrumb(&id).await;
            let inherited_ws = match svc.parent_of(&id).await {
                Some(parent) => svc.workspace_of(&parent).await.map(|w| w.name),
                None => None,
            };
            let mut blockers = Vec::new();
            for l in svc.links.incoming(&id, LinkKind::Blocks).await {
                if let Ok(bs) = svc.snapshot(&l.from).await
                    && bs.status != Status::Done
                {
                    blockers.push(bs.title);
                }
            }
            DetailPane {
                id,
                snap,
                blocked,
                breadcrumb,
                blockers,
                inherited_ws,
                scroll: prev_scroll,
            }
        };
        self.detail = Some(pane);
    }

    /// Raw (unrendered) title of the current row, Tree/List only.
    fn current_row_title(&self) -> Option<&str> {
        match &self.view {
            View::Tree | View::Help => self.items.get(self.cursor).map(|i| i.title.as_str()),
            View::List(hits) => hits.get(self.cursor).map(|h| h.task.title.as_str()),
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
        if self.ws_editor.is_some() {
            return self.handle_ws_editor_key(code, modifiers).await;
        }
        if self.ws_picker.is_some() {
            return self.handle_ws_picker_key(code).await;
        }
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
                // Esc cancels an active batch selection before it would quit.
                if !self.marked.is_empty() {
                    self.marked.clear();
                    return Ok(true);
                }
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
                self.input = Some((InputMode::AddRoot, self.scratch_input()));
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
                        let inherited = match svc.parent_of(&id).await {
                            Some(parent) => svc.workspace_of(&parent).await.map(|w| w.name),
                            None => None,
                        };
                        let workspace_display = match &snap.workspace {
                            Some(w) => w.name.clone(),
                            None => workspace_placeholder(inherited.as_deref()),
                        };
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
                                workspace_display,
                                id_display,
                            ]
                            .map(Input::from),
                            focus: 0,
                            inherited,
                            workspace: None,
                        });
                    }
                }
            }
            // Delete (tree only) — arms the confirm modal, actual delete on 'y'.
            // Targets the marked roots (whole subtrees) or the cursor task.
            Action::Delete if in_tree => {
                let roots = self.marked_roots().await;
                if !roots.is_empty() {
                    self.confirm_delete = Some(roots);
                }
            }
            // Toggle the (non-modal) details pane; it follows the selection and
            // leaves the main tree/list focused, so it works from any view.
            Action::ViewDetail => {
                self.detail_shown = !self.detail_shown;
                self.refresh_detail().await;
            }
            // Scroll the details pane's notes (pane doesn't take focus, so it
            // has its own keys). No-op when the pane is hidden.
            Action::DetailScrollDown => {
                if let Some(d) = self.detail.as_mut() {
                    d.scroll = d.scroll.saturating_add(DETAIL_SCROLL_STEP);
                }
            }
            Action::DetailScrollUp => {
                if let Some(d) = self.detail.as_mut() {
                    d.scroll = d.scroll.saturating_sub(DETAIL_SCROLL_STEP);
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
            // Quick assign: prompt for actor(s), additive — applied to every
            // marked id (children included) or the cursor task (tree only).
            Action::Assign if in_tree => {
                let ids = self.marked_ids();
                if !ids.is_empty() {
                    self.input = Some((InputMode::Assign(ids), Input::default()));
                }
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
            // Toggle the batch mark on the cursor task *and its whole subtree*
            // (tree only), then advance so repeated presses mark down the list.
            Action::ToggleMark if in_tree => {
                self.toggle_mark().await;
                self.move_cursor(true);
            }
            // Move every marked branch under the cursor task (tree only).
            Action::MoveMarked if in_tree => {
                self.move_marked().await;
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
            // Macro, equivalent to typing "Alt+Enter, then Alt+Right/Left, then
            // (if chain_add) `a`": submit the typed text as a new task (which
            // *selects* it via `submit_input`), reparent that new task, then
            // reopen the add dialog below when chaining. Empty input is a no-op
            // beyond closing the dialog, mirroring the Alt+Enter branch —
            // reparenting the still-selected existing task instead would move
            // the wrong task.
            if let Some((mode, input)) = self.input.take() {
                let trimmed = input.value().trim().to_string();
                if !trimmed.is_empty() {
                    self.submit_input(mode, trimmed).await?;
                    match action {
                        Action::ReparentIn => self.reparent_in().await?,
                        Action::ReparentOut => self.reparent_out().await?,
                        _ => unreachable!(),
                    }
                    if self.config.chain_add {
                        self.open_add_sibling().await;
                    }
                }
            }
            return Ok(true);
        }

        // Cancel: keep the add draft as a scratchpad to seed the next add
        // dialog (saving even an empty field means "clear + Esc" wipes it).
        // Handled before the `&mut input` borrow to avoid a `self.scratchpad`
        // borrow conflict.
        if code == KeyCode::Esc {
            if let Some((mode, input)) = self.input.take()
                && matches!(mode, InputMode::AddChild(_) | InputMode::AddRoot)
            {
                self.scratchpad = input.value().to_string();
            }
            return Ok(true);
        }

        // Clear the whole input buffer in one keystroke (configurable, default
        // ctrl+d). Any input mode.
        if matches!(
            self.keymap.lookup(code, modifiers),
            Some(Action::ClearInput)
        ) {
            if let Some((_, input)) = self.input.as_mut() {
                *input = Input::default();
            }
            return Ok(true);
        }

        let Some((_, input)) = &mut self.input else {
            return Ok(true);
        };
        match code {
            // Which Enter chord submits vs. inserts a newline depends on
            // `submit_on_enter` (config). Default: Alt+Enter submits, plain
            // Enter = newline (Ctrl+Enter can't be told apart from Enter
            // without the Kitty keyboard-enhancement protocol). When
            // `submit_on_enter` is on, plain Enter submits and Shift+Enter =
            // newline (mod.rs pushes the enhancement flags so Shift+Enter is
            // distinguishable on capable terminals).
            KeyCode::Enter if is_submit_chord(modifiers, self.config.submit_on_enter) => {
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
        // A successful add consumes the scratchpad draft.
        if matches!(mode, InputMode::AddChild(_) | InputMode::AddRoot) {
            self.scratchpad.clear();
        }
        let initial_status = self
            .config
            .status_order
            .first()
            .copied()
            .unwrap_or(Status::Draft);
        match mode {
            InputMode::AddChild(parent_id) => {
                let new_id = {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    svc.create(text, Some(&parent_id), initial_status, [])
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
                    svc.create(text, None, initial_status, [])
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
            InputMode::Assign(ids) => {
                let mut error = None;
                {
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    // Every actor applied to every target task (additive).
                    for actor in parse_actor_list(&text) {
                        for id in &ids {
                            if let Err(e) = svc.assign(id, actor.clone()).await {
                                error = Some(format!("assign: {e}"));
                            }
                        }
                    }
                }
                if let Some(msg) = error {
                    self.set_status(msg);
                }
                self.marked.clear();
                self.rebuild().await;
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
            KeyCode::Enter
                if form.focus == WORKSPACE_FIELD && !modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_ws_picker().await;
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
                if modifiers.contains(KeyModifiers::CONTROL)
                    && form.focus != ID_FIELD
                    && form.focus != WORKSPACE_FIELD =>
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
            _ if form.focus == ID_FIELD || form.focus == WORKSPACE_FIELD => {} // read-only
            _ => {
                if let Some(req) = text_edit_request(code, modifiers) {
                    form.fields[form.focus].handle(req);
                }
            }
        }
        Ok(true)
    }

    async fn submit_edit_form(&mut self, form: TaskEditForm) -> anyhow::Result<()> {
        let [title, notes, due, estimate, assignee, tags, _workspace, _id] =
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
            match DueSpec::parse(due) {
                Ok(spec) => Some(spec.resolve(self.clock.today())),
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
        if let Some(ws) = form.workspace.clone() {
            let unchanged = matches!(&snap, Ok(snap) if snap.workspace == ws);
            if !unchanged {
                let _ = svc.set_workspace(&form.id, ws).await;
            }
        }
        if let Err(e) = result {
            self.set_status(format!("edit: {e}"));
        }
        self.rebuild().await;
        Ok(())
    }

    async fn cycle_status(&mut self) -> anyhow::Result<()> {
        let targets = self.marked_ids();
        if targets.is_empty() {
            return Ok(());
        }
        let order = self.config.status_order.clone();
        let mut error = None;
        {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            // Each task advances from *its own* current status.
            for id in targets {
                let Ok(snap) = svc.snapshot(&id).await else {
                    continue;
                };
                let new_status = match order.iter().position(|s| *s == snap.status) {
                    Some(i) => order[(i + 1) % order.len()],
                    None => order[0],
                };
                if let Err(e) = svc.set_status(&id, new_status).await {
                    error = Some(format!("status: {e}"));
                }
            }
        }
        if let Some(msg) = error {
            self.set_status(msg);
        }
        self.marked.clear();
        self.rebuild().await;
        Ok(())
    }

    async fn claim(&mut self) -> anyhow::Result<()> {
        let targets = self.marked_ids();
        if targets.is_empty() {
            return Ok(());
        }
        // ponytail: single-user TUI — fixed actor "me"; no auth in v1 (spec §2/§13 Q5)
        let mut error = None;
        {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            for id in targets {
                if let Err(e) = svc.claim(&id, Id::new("me")).await {
                    error = Some(format!("claim: {e}"));
                }
            }
        }
        if let Some(msg) = error {
            self.set_status(msg);
        }
        self.marked.clear();
        self.rebuild().await;
        Ok(())
    }

    async fn handle_confirm_delete_key(&mut self, code: KeyCode) -> anyhow::Result<bool> {
        let Some(ids) = self.confirm_delete.take() else {
            return Ok(true);
        };
        if let KeyCode::Char('y' | 'Y') = code {
            let mut error = None;
            {
                let svc = make_svc(&self.store, &self.clock, &self.ids);
                // Roots only, recursive: each carries its own subtree.
                for id in ids {
                    if let Err(e) = svc.delete_task(&id, true).await {
                        error = Some(format!("delete: {e}"));
                    }
                }
            }
            if let Some(msg) = error {
                self.set_status(msg);
            }
            self.marked.clear();
            self.rebuild().await;
            self.cursor = self.cursor.min(self.item_count().saturating_sub(1));
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
        let input = self.scratch_input();
        self.input = Some(match self.cursor_id() {
            Some(id) => {
                let svc = make_svc(&self.store, &self.clock, &self.ids);
                match svc.parent_of(&id).await {
                    Some(parent_id) => (InputMode::AddChild(parent_id), input),
                    None => (InputMode::AddRoot, input),
                }
            }
            None => (InputMode::AddRoot, input),
        });
    }

    /// A fresh add-dialog input seeded with the scratchpad draft (empty ⇒
    /// same as `Input::default()`), cursor at end.
    fn scratch_input(&self) -> Input {
        Input::new(self.scratchpad.clone())
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

    /// Toggle the batch mark on the cursor task and its whole subtree. Reads the
    /// subtree from the store (not the visible `items`), so a collapsed branch
    /// still marks all its descendants.
    async fn toggle_mark(&mut self) {
        let Some(id) = self.cursor_id() else {
            return;
        };
        let subtree = {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            svc.descendants(&id).await
        };
        if self.marked.contains(&id) {
            self.marked.remove(&id);
            for d in subtree {
                self.marked.remove(&d);
            }
        } else {
            self.marked.insert(id);
            self.marked.extend(subtree);
        }
    }

    /// Move every marked branch (its root) under the cursor task, preserving each
    /// subtree's internal hierarchy (`move_task` carries the whole subtree). The
    /// decider rejects a move that would create a cycle — e.g. the cursor sits
    /// inside a marked subtree — which is surfaced as a status message.
    async fn move_marked(&mut self) {
        let Some(parent) = self.cursor_id() else {
            return;
        };
        let roots = self.marked_roots().await;
        let mut error = None;
        {
            let svc = make_svc(&self.store, &self.clock, &self.ids);
            for id in roots {
                if id == parent {
                    continue;
                }
                if let Err(e) = svc.move_task(&id, &parent, None).await {
                    error = Some(format!("move: {e}"));
                }
            }
        }
        if let Some(msg) = error {
            self.set_status(msg);
        }
        self.expanded.insert(parent.clone());
        self.marked.clear();
        self.rebuild().await;
        if let Some(pos) = self.items.iter().position(|i| i.id == parent) {
            self.cursor = pos;
        }
    }

    /// Opens the workspace-assignment popup on the edit form's workspace
    /// field: every known workspace, then the inherited/none entry (same
    /// action either way — unassign), then "(new…)".
    async fn open_ws_picker(&mut self) {
        let Some(form) = &self.edit_form else {
            return;
        };
        let svc = make_svc(&self.store, &self.clock, &self.ids);
        let workspaces = svc.workspaces().await;
        let mut items: Vec<WsPickerItem> = workspaces
            .into_iter()
            .map(|(name, (path, _ids))| {
                let label = match &path {
                    Some(p) => format!("{name} ({p})"),
                    None => name.clone(),
                };
                WsPickerItem {
                    label,
                    action: WsPickerAction::Assign(Workspace { name, path }),
                }
            })
            .collect();
        items.push(WsPickerItem {
            label: workspace_placeholder(form.inherited.as_deref()),
            action: WsPickerAction::Unassign,
        });
        items.push(WsPickerItem {
            label: "(new…)".to_string(),
            action: WsPickerAction::New,
        });
        self.ws_picker = Some(WsPicker { items, selected: 0 });
    }

    /// Stages a workspace change on the open edit form (no store write until
    /// save) and updates the field's display text to match.
    fn stage_workspace(&mut self, ws: Option<Workspace>) {
        let Some(form) = &mut self.edit_form else {
            return;
        };
        let text = match &ws {
            Some(w) => w.name.clone(),
            None => workspace_placeholder(form.inherited.as_deref()),
        };
        form.fields[WORKSPACE_FIELD] = Input::from(text);
        form.workspace = Some(ws);
    }

    async fn handle_ws_picker_key(&mut self, code: KeyCode) -> anyhow::Result<bool> {
        let Some(picker) = &mut self.ws_picker else {
            return Ok(true);
        };
        match code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(picker.items.len().saturating_sub(1));
            }
            KeyCode::Esc => self.ws_picker = None,
            KeyCode::Enter => {
                if let Some(picker) = self.ws_picker.take()
                    && let Some(item) = picker.items.into_iter().nth(picker.selected)
                {
                    match item.action {
                        WsPickerAction::Assign(ws) => self.stage_workspace(Some(ws)),
                        WsPickerAction::Unassign => self.stage_workspace(None),
                        WsPickerAction::New => self.open_ws_editor(true).await,
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// Opens the workspace editor: one row per known workspace (name/default
    /// path/config override) plus a fresh row focused on its name cell.
    async fn open_ws_editor(&mut self, assign: bool) {
        let overrides = todoapp_config::workspace_overrides();
        let svc = make_svc(&self.store, &self.clock, &self.ids);
        let mut rows: Vec<WsRow> = svc
            .workspaces()
            .await
            .into_iter()
            .map(|(name, (path, _ids))| {
                let cfg_override = overrides.get(&name).cloned();
                WsRow {
                    name,
                    db_path: path,
                    override_: cfg_override,
                    is_new: false,
                }
            })
            .collect();
        rows.push(WsRow {
            name: String::new(),
            db_path: None,
            override_: None,
            is_new: true,
        });
        let new_row = rows.len() - 1;
        self.ws_editor = Some(WsEditor {
            rows,
            cursor: (new_row, 0),
            editing: Some(Input::default()),
            assign,
        });
    }

    async fn handle_ws_editor_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<bool> {
        let Some(mut editor) = self.ws_editor.take() else {
            return Ok(true);
        };
        match code {
            KeyCode::Up if editor.editing.is_none() => {
                editor.cursor.0 = editor.cursor.0.saturating_sub(1);
            }
            KeyCode::Down if editor.editing.is_none() => {
                editor.cursor.0 = (editor.cursor.0 + 1).min(editor.rows.len().saturating_sub(1));
            }
            KeyCode::Left if editor.editing.is_none() => {
                editor.cursor.1 = editor.cursor.1.saturating_sub(1);
            }
            KeyCode::Right if editor.editing.is_none() => {
                editor.cursor.1 = (editor.cursor.1 + 1).min(2);
            }
            KeyCode::Enter if editor.editing.is_none() => {
                let (r, c) = editor.cursor;
                let current = match (c, editor.rows.get(r)) {
                    (0, Some(row)) => row.name.clone(),
                    (1, Some(row)) => row.db_path.clone().unwrap_or_default(),
                    (_, Some(row)) => row.override_.clone().unwrap_or_default(),
                    (_, None) => String::new(),
                };
                editor.editing = Some(Input::from(current));
            }
            KeyCode::Enter => {
                if let Some(input) = editor.editing.take() {
                    self.commit_ws_cell(&mut editor, input.value().to_string())
                        .await;
                }
            }
            KeyCode::Esc if editor.editing.is_some() => {
                editor.editing = None;
            }
            KeyCode::Esc => {
                if editor.assign
                    && let Some(new_row) = editor.rows.iter().find(|r| r.is_new)
                    && !new_row.name.trim().is_empty()
                {
                    let ws = Workspace {
                        name: new_row.name.trim().to_string(),
                        path: new_row.db_path.clone(),
                    };
                    self.stage_workspace(Some(ws));
                }
                self.rebuild().await;
                return Ok(true);
            }
            _ => {
                if let Some(input) = &mut editor.editing
                    && let Some(req) = text_edit_request(code, modifiers)
                {
                    input.handle(req);
                }
            }
        }
        self.ws_editor = Some(editor);
        Ok(true)
    }

    /// Commits the cell currently being edited: persists the change for an
    /// existing row (rename/path/override), or just updates the in-memory
    /// row for the not-yet-assigned new row.
    async fn commit_ws_cell(&mut self, editor: &mut WsEditor, value: String) {
        let (r, c) = editor.cursor;
        let Some(row) = editor.rows.get(r) else {
            return;
        };
        let is_new = row.is_new;
        match c {
            0 => {
                let old_name = row.name.clone();
                let new_name = value.trim().to_string();
                if is_new {
                    editor.rows[r].name = new_name;
                } else if !new_name.is_empty() && new_name != old_name {
                    let ws = Workspace {
                        name: new_name.clone(),
                        path: row.db_path.clone(),
                    };
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    let _ = svc.update_workspace(&old_name, ws).await;
                    if let Some(over) = row.override_.clone() {
                        let _ = todoapp_config::set_workspace_override(&old_name, None);
                        let _ = todoapp_config::set_workspace_override(&new_name, Some(&over));
                    }
                    editor.rows[r].name = new_name;
                }
            }
            1 => {
                let new_path = (!value.trim().is_empty()).then(|| value.trim().to_string());
                if is_new {
                    editor.rows[r].db_path = new_path;
                } else {
                    let ws = Workspace {
                        name: row.name.clone(),
                        path: new_path.clone(),
                    };
                    let svc = make_svc(&self.store, &self.clock, &self.ids);
                    let _ = svc.update_workspace(&row.name.clone(), ws).await;
                    editor.rows[r].db_path = new_path;
                }
            }
            _ => {
                let new_override = (!value.trim().is_empty()).then(|| value.trim().to_string());
                if !row.name.is_empty() {
                    let _ = todoapp_config::set_workspace_override(
                        &row.name.clone(),
                        new_override.as_deref(),
                    );
                }
                editor.rows[r].override_ = new_override;
            }
        }
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

    #[test]
    fn submit_chord_depends_on_config() {
        // Default (Alt+Enter submits, plain Enter = newline).
        assert!(is_submit_chord(KeyModifiers::ALT, false));
        assert!(!is_submit_chord(KeyModifiers::NONE, false));
        // submit_on_enter (plain Enter submits, Shift+Enter = newline).
        assert!(is_submit_chord(KeyModifiers::NONE, true));
        assert!(!is_submit_chord(KeyModifiers::SHIFT, true));
    }

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
            Box::new(crate::tui::clipboard::FakeClipboard::default()),
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

    /// New tasks must start on `status_order[0]`, not a hardcoded `Draft` —
    /// otherwise a task created while `draft` is excluded from
    /// `status.enabled` would land outside the configured cycle.
    #[tokio::test]
    async fn new_task_starts_on_first_enabled_status() {
        let mut app = new_app().await; // default config: enabled = [todo, wip, paused, done]
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "Task").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        let item = app.items.iter().find(|i| i.title == "Task").unwrap();
        assert_eq!(item.status, Status::Todo);
    }

    /// The quick-assign action (`s`) is additive: it doesn't clear an
    /// assignee already set (e.g. via an `@mention` on create).
    #[tokio::test]
    async fn quick_assign_action_adds_actor_without_clearing_existing() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "fix @alice bug").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        let id = app
            .items
            .iter()
            .find(|i| i.title == "fix bug")
            .unwrap()
            .id
            .clone();
        app.cursor = app.items.iter().position(|i| i.id == id).unwrap();

        app.handle_event(press_char('s'), TERM_WIDTH).await.unwrap();
        assert!(
            matches!(&app.input, Some((InputMode::Assign(target), _)) if *target == vec![id.clone()])
        );
        type_str(&mut app, "bob").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        let snap = svc.snapshot(&id).await.unwrap();
        let actors: Vec<Id> = snap.assignments.iter().map(|a| a.actor.clone()).collect();
        assert!(actors.contains(&Id::new("alice")));
        assert!(actors.contains(&Id::new("bob")));
    }

    /// With `draft` re-added to `status.enabled` as the first entry, new
    /// tasks are created as `draft` again.
    #[tokio::test]
    async fn new_task_starts_as_draft_when_draft_is_first_enabled() {
        let mut app = new_app_with(
            None,
            Some("[status]\nenabled = [\"draft\", \"todo\", \"wip\", \"paused\", \"done\"]\n"),
        )
        .await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "Task").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        let item = app.items.iter().find(|i| i.title == "Task").unwrap();
        assert_eq!(item.status, Status::Draft);
    }

    async fn new_app_with(keymap_toml: Option<&str>, config_toml: Option<&str>) -> AppState {
        let keymap_toml = keymap_toml.map(|s| toml::from_str(s).expect("valid TOML"));
        let config_toml = config_toml.map(|s| toml::from_str(s).expect("valid TOML"));
        AppState::new(
            TursoStore::open_memory().await,
            Keymap::load(keymap_toml.as_ref()).unwrap(),
            Config::load(config_toml.as_ref()).unwrap(),
            Box::new(crate::tui::clipboard::FakeClipboard::default()),
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

    /// Cancelling (Esc) an add dialog keeps the typed text as a scratchpad,
    /// which seeds the next add dialog instead of starting empty.
    #[tokio::test]
    async fn cancelled_add_draft_is_kept_and_seeds_next_dialog() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "draft").await;
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.input.is_none(), "Esc closes the dialog");
        assert_eq!(app.scratchpad, "draft", "draft kept on cancel");

        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "draft", "next dialog seeded with the draft");
    }

    /// A successful add consumes the scratchpad, so the next (or chained)
    /// dialog starts empty.
    #[tokio::test]
    async fn submit_consumes_scratchpad() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "Task").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.scratchpad, "", "submit clears the scratchpad");

        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "", "reopened dialog is empty after submit");
    }

    /// Clearing the field then Esc leaves an empty scratchpad — a free "clear
    /// the draft".
    #[tokio::test]
    async fn clear_field_then_esc_wipes_scratchpad() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "stale").await;
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.scratchpad, "stale");

        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap(); // seeded "stale"
        app.handle_event(press(KeyCode::Char('d'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap(); // clear_input
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.scratchpad, "", "cleared field + Esc wipes the draft");
    }

    /// `clear_input` (default ctrl+d) empties the whole buffer in one keystroke
    /// without closing the dialog; the chord is read live from the keymap.
    #[tokio::test]
    async fn clear_input_key_empties_field() {
        let mut app = new_app().await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "hello world").await;
        app.handle_event(press(KeyCode::Char('d'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "", "ctrl+d clears the buffer");

        // Rebound chord (ctrl+l) drives the same clear.
        let mut app = new_app_with(Some("[keybindings]\nclear_input = [\"ctrl+l\"]\n"), None).await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "hello").await;
        app.handle_event(press(KeyCode::Char('l'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        let (_, input) = app.input.as_ref().unwrap();
        assert_eq!(input.value(), "", "rebound clear_input works");
    }

    /// Alt+Right/Alt+Left inside the add dialog is a macro: it *submits* the
    /// typed text as a new task (which selects it), then reparents that new
    /// task — like "Alt+Enter, then Alt+Right/Left". The reparent hits the
    /// newly-added task (not the previously-selected parent), and the typed
    /// text is never dropped.
    #[tokio::test]
    async fn depth_shift_in_add_dialog_submits_then_reparents_new_task() {
        let mut app = new_app().await; // chain_add = false (default)
        // Seed a root task A.
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "A"), Some(0));

        // Type "B" and alt+right in one chord: B is created (text not lost) and
        // nested under A; A itself does not move.
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Right, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(1), "B created and nested under A");
        assert_eq!(depth_of(&app, "A"), Some(0), "A (the parent) did not move");
        assert!(app.input.is_none(), "chain_add off: dialog closes");

        // Type "C" and alt+left: C is created (child of A) then outdented to root.
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "C").await;
        app.handle_event(press(KeyCode::Left, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(
            depth_of(&app, "C"),
            Some(0),
            "C created then outdented to root"
        );
    }

    /// With `chain_add`, the depth-shift macro also reopens the add dialog
    /// below afterward — mirroring the trailing `a` in "Alt+Enter, Alt+Right, a".
    #[tokio::test]
    async fn depth_shift_in_add_dialog_reopens_when_chaining() {
        let mut app = new_app_with(None, Some("[behavior]\nchain_add = true\n")).await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();

        // Dialog stays open (chaining); type B then alt+right.
        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Right, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(1)); // B created and nested under A
        assert!(
            app.input.is_some(),
            "chain_add on: add dialog reopens below"
        );
        let (mode, _) = app.input.as_ref().unwrap();
        assert!(matches!(mode, InputMode::AddChild(_))); // sibling-of-B target (child of A)
    }

    /// `reparent_in`'s chord is read live from the keymap, not hardcoded —
    /// rebinding it still drives the same submit-then-reparent macro.
    #[tokio::test]
    async fn depth_shift_honors_rebound_chord() {
        let mut app = new_app_with(Some("[keybindings]\nreparent_in = [\"ctrl+j\"]\n"), None).await;
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "A").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();

        // Type B and trigger the *rebound* reparent_in chord (ctrl+j) mid-dialog.
        app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
        type_str(&mut app, "B").await;
        app.handle_event(press(KeyCode::Char('j'), KeyModifiers::CONTROL), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(depth_of(&app, "B"), Some(1)); // rebound chord created B and nested it under A
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

    /// Tabs the open edit form forward to the workspace field.
    async fn tab_to_workspace_field(app: &mut AppState) {
        for _ in 0..WORKSPACE_FIELD {
            app.handle_event(press(KeyCode::Tab, KeyModifiers::NONE), TERM_WIDTH)
                .await
                .unwrap();
        }
        assert_eq!(app.edit_form.as_ref().unwrap().focus, WORKSPACE_FIELD);
    }

    #[tokio::test]
    async fn workspace_picker_assigns_existing_workspace() {
        let (mut app, task_id) = new_app_with_task().await;
        let ws = Workspace {
            name: "proj".into(),
            path: Some("/x".into()),
        };
        {
            let other = {
                let svc = make_svc(&app.store, &app.clock, &app.ids);
                svc.create("Proj", None, Status::Todo, []).await.unwrap().id
            };
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            svc.set_workspace(&other, Some(ws.clone())).await.unwrap();
        }

        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        tab_to_workspace_field(&mut app).await;

        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.ws_picker.is_some());
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.ws_picker.is_none());
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD].value(),
            "proj"
        );

        let snap = save_edit_form(&mut app, &task_id).await;
        assert_eq!(snap.workspace, Some(ws));
    }

    #[tokio::test]
    async fn restore_reapplies_expansion_and_cursor() {
        let mut app = new_app().await;
        let (root_id, child_id) = {
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            let root = svc.create("Root", None, Status::Todo, []).await.unwrap();
            let child = svc
                .create("Child", Some(&root.id), Status::Todo, [])
                .await
                .unwrap();
            (root.id, child.id)
        };
        // Collapsed by default: only the root is visible, cursor at the top.
        app.rebuild().await;
        assert_eq!(app.items.len(), 1);

        let mut state = crate::tui::state::UiState::default();
        state.expanded.insert(root_id);
        state.cursor = Some(child_id.clone());
        state.detail_shown = true;
        app.restore(state).await;

        // Root expanded (child now visible), cursor on the child, pane restored.
        assert_eq!(app.items.len(), 2);
        assert_eq!(app.cursor_id(), Some(child_id));
        assert!(app.detail_shown);

        // A stale cursor id (deleted since) is ignored — the cursor stays put
        // rather than jumping, and nothing panics.
        let before = app.cursor;
        let mut gone = app.ui_state();
        gone.cursor = Some(Id::new("does-not-exist"));
        app.restore(gone).await;
        assert_eq!(app.cursor, before);
    }

    #[tokio::test]
    async fn workspace_picker_inherited_or_none_stages_unassign() {
        let mut app = new_app().await;
        let (root_id, child_id) = {
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            let root = svc.create("Root", None, Status::Todo, []).await.unwrap();
            let ws = Workspace {
                name: "root-ws".into(),
                path: None,
            };
            svc.set_workspace(&root.id, Some(ws)).await.unwrap();
            let child = svc
                .create("Child", Some(&root.id), Status::Todo, [])
                .await
                .unwrap();
            (root.id, child.id)
        };
        app.expanded.insert(root_id);
        app.rebuild().await;
        app.cursor = app.items.iter().position(|i| i.id == child_id).unwrap();

        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD].value(),
            "(inherited: root-ws)"
        );
        tab_to_workspace_field(&mut app).await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // open picker: [root-ws, inherited/none, new…]
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // select inherited/none

        let snap = save_edit_form(&mut app, &child_id).await;
        assert_eq!(snap.workspace, None);
        let svc = make_svc(&app.store, &app.clock, &app.ids);
        assert_eq!(
            svc.workspace_of(&child_id).await.map(|w| w.name),
            Some("root-ws".to_string())
        );
    }

    #[tokio::test]
    async fn workspace_new_via_editor_stages_and_saves() {
        let (mut app, task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        tab_to_workspace_field(&mut app).await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // open picker: [inherited/none, new…]
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // select "(new…)"
        {
            let editor = app.ws_editor.as_ref().unwrap();
            assert_eq!(editor.cursor, (0, 0));
            assert!(editor.editing.is_some());
        }
        type_str(&mut app, "newproj").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // commit name
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // close editor -> stages onto the edit form
        assert!(app.ws_editor.is_none());
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD].value(),
            "newproj"
        );

        let snap = save_edit_form(&mut app, &task_id).await;
        assert_eq!(snap.workspace.map(|w| w.name), Some("newproj".to_string()));
    }

    #[tokio::test]
    async fn workspace_esc_from_form_discards_staged_change() {
        let (mut app, task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        tab_to_workspace_field(&mut app).await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // select "(new…)"
        type_str(&mut app, "abandoned").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // close editor, stages "abandoned"
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD].value(),
            "abandoned"
        );

        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // Esc from form: cancel everything
        assert!(app.edit_form.is_none());

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        let snap = svc.snapshot(&task_id).await.unwrap();
        assert_eq!(snap.workspace, None);
    }

    #[tokio::test]
    async fn workspace_editor_rename_propagates_to_all_carriers() {
        let mut app = new_app().await;
        let (id_a, id_b) = {
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            let a = svc.create("A", None, Status::Todo, []).await.unwrap();
            let b = svc.create("B", None, Status::Todo, []).await.unwrap();
            let ws = Workspace {
                name: "old".into(),
                path: Some("/p".into()),
            };
            svc.set_workspace(&a.id, Some(ws.clone())).await.unwrap();
            svc.set_workspace(&b.id, Some(ws)).await.unwrap();
            (a.id, b.id)
        };
        app.rebuild().await;
        app.cursor = app.items.iter().position(|i| i.id == id_a).unwrap();

        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        tab_to_workspace_field(&mut app).await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // open picker: [old, inherited/none, new…]
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // select "(new…)": editor opens, new row's name cell mid-edit

        // Cancel that cell's edit so arrows can navigate to the existing row.
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Up, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // start editing "old"'s name cell
        for _ in 0.."old".len() {
            app.handle_event(press(KeyCode::Backspace, KeyModifiers::NONE), TERM_WIDTH)
                .await
                .unwrap();
        }
        type_str(&mut app, "renamed").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // commit rename
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap(); // close editor (new row still unnamed -> nothing staged)

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        let snap_b = svc.snapshot(&id_b).await.unwrap();
        assert_eq!(
            snap_b.workspace.map(|w| w.name),
            Some("renamed".to_string())
        );
    }

    #[tokio::test]
    async fn workspace_field_is_read_only_except_via_picker() {
        let (mut app, _task_id) = new_app_with_task().await;
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        tab_to_workspace_field(&mut app).await;
        let before = app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD]
            .value()
            .to_string();
        type_str(&mut app, "xyz").await;
        assert_eq!(
            app.edit_form.as_ref().unwrap().fields[WORKSPACE_FIELD].value(),
            before
        );
    }

    /// The details pane is a non-modal toggle: `v` shows/hides it, it follows
    /// the selection (the run-loop refresh, mirrored here), scrolls with
    /// PageUp/Down (resetting on task change), and leaves the tree focused so
    /// editing still works while it's open.
    #[tokio::test]
    async fn details_pane_toggles_follows_selection_and_scrolls() {
        let mut app = new_app().await;
        for title in ["First", "Second"] {
            app.handle_event(press_char('a'), TERM_WIDTH).await.unwrap();
            type_str(&mut app, title).await;
            app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
                .await
                .unwrap();
        }
        app.handle_event(press(KeyCode::Home, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        let first_id = app.selected_id().unwrap();
        let first_title = app.items[app.cursor].title.clone();

        // Toggle on: pane appears and caches the selected task.
        app.handle_event(press_char('v'), TERM_WIDTH).await.unwrap();
        assert!(app.detail_shown);
        assert_eq!(app.detail.as_ref().unwrap().id, first_id);
        assert_eq!(app.detail.as_ref().unwrap().snap.title, first_title);

        // Scroll the pane (its own keys, doesn't move the selection).
        app.handle_event(press(KeyCode::PageDown, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert_eq!(app.detail.as_ref().unwrap().scroll, DETAIL_SCROLL_STEP);
        assert_eq!(app.selected_id().unwrap(), first_id);

        // Navigate: the run-loop refreshes the pane after the keystroke.
        app.handle_event(press(KeyCode::Down, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        app.refresh_detail().await; // mirrors run_loop
        let second_id = app.selected_id().unwrap();
        let second_title = app.items[app.cursor].title.clone();
        assert_ne!(first_id, second_id);
        assert_eq!(app.detail.as_ref().unwrap().id, second_id);
        assert_eq!(app.detail.as_ref().unwrap().snap.title, second_title);
        assert_eq!(app.detail.as_ref().unwrap().scroll, 0); // reset on task change

        // Pane doesn't steal focus: editing still works while it's shown.
        app.handle_event(press_char('e'), TERM_WIDTH).await.unwrap();
        assert!(app.edit_form.is_some());
        assert!(app.detail_shown);
        app.handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();

        // Toggle off.
        app.handle_event(press_char('v'), TERM_WIDTH).await.unwrap();
        assert!(!app.detail_shown);
        assert!(app.detail.is_none());
    }

    /// Build a fixed tree for the multi-select tests: a branch `P` with two
    /// children `C1`/`C2`, plus a standalone sibling `S` (the move target).
    /// Returns `(p, c1, c2, s)`. `P` stays collapsed (nothing added to
    /// `expanded`) so tests also cover marking a collapsed subtree.
    async fn tree_p_children_s(app: &mut AppState) -> (Id, Id, Id, Id) {
        let ids = {
            let svc = make_svc(&app.store, &app.clock, &app.ids);
            let p = svc.create("P", None, Status::Todo, []).await.unwrap().id;
            let c1 = svc
                .create("C1", Some(&p), Status::Todo, [])
                .await
                .unwrap()
                .id;
            let c2 = svc
                .create("C2", Some(&p), Status::Todo, [])
                .await
                .unwrap()
                .id;
            let s = svc.create("S", None, Status::Todo, []).await.unwrap().id;
            (p, c1, c2, s)
        };
        app.rebuild().await;
        ids
    }

    fn cursor_to(app: &mut AppState, id: &Id) {
        app.cursor = app.items.iter().position(|i| &i.id == id).unwrap();
    }

    /// `m` marks a task *and its whole subtree* (even collapsed), and `p` moves
    /// the marked branch under the cursor task while keeping its children.
    #[tokio::test]
    async fn mark_selects_whole_subtree_and_move_preserves_hierarchy() {
        let mut app = new_app().await;
        let (p, c1, c2, s) = tree_p_children_s(&mut app).await;

        // Mark the (collapsed) branch P.
        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        assert_eq!(
            app.marked,
            HashSet::from([p.clone(), c1.clone(), c2.clone()]),
            "marking P must recursively mark its subtree even when collapsed"
        );
        // Roots = just P; the full set = the whole subtree.
        assert_eq!(app.marked_roots().await, vec![p.clone()]);
        assert_eq!(app.marked_ids().len(), 3);

        // Move the marked branch under S.
        cursor_to(&mut app, &s);
        app.handle_event(press_char('p'), TERM_WIDTH).await.unwrap();
        assert!(app.marked.is_empty(), "marks cleared after move");

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        assert_eq!(svc.parent_of(&p).await.as_ref(), Some(&s));
        assert_eq!(svc.parent_of(&c1).await.as_ref(), Some(&p));
        assert_eq!(svc.parent_of(&c2).await.as_ref(), Some(&p));
    }

    /// `m` toggles off (removes the subtree); `esc` clears an active selection
    /// instead of quitting.
    #[tokio::test]
    async fn mark_toggle_off_and_esc_clears() {
        let mut app = new_app().await;
        let (p, _c1, _c2, _s) = tree_p_children_s(&mut app).await;

        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.marked.len(), 3);
        // Toggle the same branch off.
        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        assert!(app.marked.is_empty(), "second m unmarks the whole subtree");

        // Re-mark, then esc clears without quitting.
        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        assert_eq!(app.marked.len(), 3);
        let keep_running = app
            .handle_event(press(KeyCode::Esc, KeyModifiers::NONE), TERM_WIDTH)
            .await
            .unwrap();
        assert!(keep_running, "esc must not quit while marks are active");
        assert!(app.marked.is_empty());
    }

    /// Batch assign applies the actor to every marked id — parent and children.
    #[tokio::test]
    async fn batch_assign_reaches_all_descendants() {
        let mut app = new_app().await;
        let (p, c1, c2, _s) = tree_p_children_s(&mut app).await;

        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        // `s` = assign; prompt should target the whole marked set.
        app.handle_event(press_char('s'), TERM_WIDTH).await.unwrap();
        assert!(matches!(&app.input, Some((InputMode::Assign(t), _)) if t.len() == 3));
        type_str(&mut app, "bot").await;
        app.handle_event(press(KeyCode::Enter, KeyModifiers::ALT), TERM_WIDTH)
            .await
            .unwrap();
        assert!(app.marked.is_empty());

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        for id in [&p, &c1, &c2] {
            let snap = svc.snapshot(id).await.unwrap();
            assert!(
                snap.assignments.iter().any(|a| a.actor == Id::new("bot")),
                "every task in the marked branch should carry the actor"
            );
        }
    }

    /// Batch status-cycle advances each marked task from its *own* status.
    #[tokio::test]
    async fn batch_cycle_status_advances_each_marked_task() {
        let mut app = new_app().await;
        let (p, c1, c2, _s) = tree_p_children_s(&mut app).await;

        cursor_to(&mut app, &p);
        app.handle_event(press_char('m'), TERM_WIDTH).await.unwrap();
        // space = cycle_status; default order starts todo -> wip.
        app.handle_event(press_char(' '), TERM_WIDTH).await.unwrap();
        assert!(app.marked.is_empty());

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        for id in [&p, &c1, &c2] {
            assert_eq!(svc.snapshot(id).await.unwrap().status, Status::Wip);
        }
    }
}
