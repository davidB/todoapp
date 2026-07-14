//! Pure rendering: takes `&AppState`, draws to `Frame`. No mutations.

use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
    },
};
use todoapp_core::Status;

use crate::tui::app::{
    AppState, DetailPane, ID_FIELD, InputMode, NOTES_FIELD, Selection, TITLE_FIELD, View,
    VisibleItem, WsEditor, WsPicker,
};
use crate::tui::config::{ColumnKind, Config, Semantic};
use crate::tui::keymap::{Action, Keymap};
use crate::tui::text_edit;

pub fn render(f: &mut Frame, app: &AppState) {
    let area = f.area();
    let [main, status_bar] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);

    // The details pane (when toggled on) is a non-modal, full-width strip below
    // the main view — the tree/list keeps focus and the top of the screen.
    let content = if app.detail_shown {
        let [top, bottom] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(main);
        render_detail(f, bottom, app);
        top
    } else {
        main
    };
    render_tree(f, content, app);
    if let View::List(hits) = &app.view {
        render_list(f, content, app, hits);
    }

    render_status_bar(f, status_bar, app);

    if let Some((mode, input)) = &app.input {
        render_input_modal(f, area, mode, input);
    }
    if let Some((id, has_children)) = &app.confirm_delete {
        render_confirm_delete_modal(f, area, app, id, *has_children);
    }
    if let Some(form) = &app.edit_form {
        render_edit_form(f, area, form);
    }
    if let Some(picker) = &app.ws_picker {
        render_ws_picker(f, area, picker);
    }
    if let Some(editor) = &app.ws_editor {
        render_ws_editor(f, area, editor);
    }
    if matches!(app.view, View::Help) {
        render_help(f, area, &app.keymap);
    }
}

/// The tree column (indent + expand arrow + status icon + title + blocked
/// badge) plus one configured column per capability (spec: values are always
/// the aggregate over the task + its descendants, `Services::aggregate`).
fn render_tree(f: &mut Frame, area: Rect, app: &AppState) {
    // The tree (title) column is the most important part of the view: it
    // always keeps at least 30% of the area's width. If the configured
    // columns don't leave that much room, drop columns from the right
    // (lowest priority = last in configured order) until they do.
    // ponytail: hides columns rather than adding a horizontal scrollbar —
    // ratatui's Table has no native scroll support, and this is simpler.
    let min_tree_width = area.width * 30 / 100;
    let mut columns: &[ColumnKind] = &app.config.columns;
    while !columns.is_empty() && tree_col_width_for(area.width, columns) < min_tree_width {
        columns = &columns[..columns.len() - 1];
    }
    let header = Row::new(
        std::iter::once(Cell::from("tree")).chain(columns.iter().map(|c| Cell::from(c.header()))),
    );
    // Approximate the tree (title) column's rendered width: total width minus
    // the block's left/right border, the other configured columns, and one
    // column-spacing gap per configured column (Table's default spacing).
    // ponytail: an approximation, not ratatui's exact layout math — only
    // used to size the ellipsis truncation, a char or two off doesn't matter.
    let tree_col_width = tree_col_width_for(area.width, columns);
    let rows: Vec<Row> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| item_row(item, columns, app, tree_col_width, i == app.cursor))
        .collect();

    let mut widths = vec![Constraint::Fill(1)];
    widths.extend(columns.iter().map(|c| Constraint::Length(column_width(*c))));

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" tda "))
        .row_highlight_style(app.config.style_for(Semantic::Selected));
    let mut state = TableState::default().with_selected(Some(app.cursor));
    f.render_stateful_widget(table, area, &mut state);
}

fn tree_col_width_for(area_width: u16, columns: &[ColumnKind]) -> u16 {
    let configured_width: u16 = columns.iter().map(|c| column_width(*c)).sum();
    let gaps = u16::try_from(columns.len()).unwrap_or(0);
    area_width.saturating_sub(2 + configured_width + gaps)
}

fn column_width(kind: ColumnKind) -> u16 {
    match kind {
        // `PROGRESS_BAR_WIDTH` (6) + " " + up to "999/999" (7), so subtree
        // counts up to 3 digits each don't get silently truncated.
        ColumnKind::Status => 14,
        ColumnKind::Due | ColumnKind::Eta => 10,
        ColumnKind::Assignee | ColumnKind::Tags => 16,
        ColumnKind::Estimate | ColumnKind::Elapsed | ColumnKind::Id => 8,
    }
}

fn item_row(
    item: &VisibleItem,
    columns: &[ColumnKind],
    app: &AppState,
    tree_col_width: u16,
    is_cursor: bool,
) -> Row<'static> {
    let tree_cell = Cell::from(tree_cell_line(item, app, tree_col_width, is_cursor));
    let cells =
        std::iter::once(tree_cell).chain(columns.iter().map(|c| render_column(item, *c, app)));
    Row::new(cells)
}

/// The status text style for `item`'s title only — `AggregateText` (dimmed)
/// for rows summarizing a subtree, `Text` for a leaf task. Deliberately not
/// applied to the whole row: only the title should look muted/crossed-out
/// for e.g. `Done`, not the due/eta/assignee/... columns alongside it.
fn title_style(item: &VisibleItem, app: &AppState) -> Style {
    let semantic = if item.has_children {
        Semantic::AggregateText(item.agg_status)
    } else {
        Semantic::Text(item.status)
    };
    app.config.style_for(semantic)
}

fn tree_cell_line(
    item: &VisibleItem,
    app: &AppState,
    tree_col_width: u16,
    is_cursor: bool,
) -> Line<'static> {
    let indent = "  ".repeat(item.depth);
    let arrow = if item.has_children {
        if item.is_expanded { "▼ " } else { "▶ " }
    } else {
        "· "
    };
    let icon = status_icon(item.status, app);
    let prefix = format!("{indent}{arrow}");
    let badge_width = if item.is_blocked { 4 } else { 0 }; // " [!]"
    let title_width = usize::from(tree_col_width)
        .saturating_sub(prefix.chars().count())
        .saturating_sub(icon.chars().count() + 1)
        .saturating_sub(badge_width)
        .max(1);
    let mut spans = vec![
        Span::raw(prefix),
        Span::styled(icon, app.config.style_for(Semantic::Glyph(item.status))),
        Span::raw(" "),
    ];
    let title_base = title_style(item, app);
    let title_spans = match (is_cursor, app.selection) {
        (true, Some(sel)) => selection_spans(&item.title, title_width, sel),
        _ => crate::tui::markdown::render_inline(&item.title, title_width),
    };
    spans.extend(
        title_spans
            .into_iter()
            .map(|s| Span::styled(s.content, title_base.patch(s.style))),
    );
    if item.is_blocked {
        spans.push(Span::raw(" [!]"));
    }
    Line::from(spans)
}

/// ponytail: plain (non-Markdown) first line of `title`, clipped to `width`
/// chars, with the inclusive `sel` char range reversed — this is only shown
/// on the cursor row while actively selecting, so skipping Markdown rendering
/// and ellipsis truncation here is a deliberate simplification, not a gap.
fn selection_spans(title: &str, width: usize, sel: Selection) -> Vec<Span<'static>> {
    let first_line = title.split('\n').next().unwrap_or("");
    let chars: Vec<char> = first_line.chars().take(width.max(1)).collect();
    let start = sel.anchor.min(sel.cursor).min(chars.len());
    let end = sel
        .anchor
        .max(sel.cursor)
        .min(chars.len().saturating_sub(1));
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::raw(chars[..start].iter().collect::<String>()));
    }
    if start < chars.len() {
        spans.push(Span::styled(
            chars[start..=end.max(start)].iter().collect::<String>(),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    if end + 1 < chars.len() {
        spans.push(Span::raw(chars[end + 1..].iter().collect::<String>()));
    }
    spans
}

fn render_column(item: &VisibleItem, kind: ColumnKind, app: &AppState) -> Cell<'static> {
    match kind {
        ColumnKind::Status => Cell::from(progress_bar(
            &item.by_status,
            item.done,
            item.total,
            &app.config,
        )),
        ColumnKind::Due => Cell::from(item.due.map_or("-".to_string(), |d| d.to_string())),
        ColumnKind::Eta => match item.eta {
            Some((date, overrun)) => {
                let cell = Cell::from(date.to_string());
                if overrun {
                    cell.style(app.config.style_for(Semantic::Overdue))
                } else {
                    cell
                }
            }
            None => Cell::from("-"),
        },
        ColumnKind::Assignee => Cell::from(if item.assignees.is_empty() {
            "-".to_string()
        } else {
            item.assignees.clone()
        }),
        ColumnKind::Estimate => Cell::from(crate::tui::human_duration::format(
            item.estimate,
            app.config.hours_per_day,
            app.config.days_per_week,
        )),
        ColumnKind::Elapsed => Cell::from(item.elapsed.to_string()),
        ColumnKind::Tags => Cell::from(if item.tags.is_empty() {
            "-".to_string()
        } else {
            item.tags.clone()
        }),
        ColumnKind::Id => Cell::from(
            app.short_ids
                .get(&item.id)
                .cloned()
                .unwrap_or_else(|| item.id.to_string()),
        ),
    }
}

const PROGRESS_BAR_WIDTH: usize = 6;

/// A small `[######] d/t` progress bar: one proportional segment per
/// `Status` in the subtree, ordered Done..Draft (finished work at the left),
/// colored with that status's glyph color so the bar and the glyphs read as
/// one consistent palette. Blocked tasks aren't called out separately — a
/// blocked task is still a Draft/Todo/Paused task and renders as such.
fn progress_bar(
    by_status: &BTreeMap<Status, usize>,
    done: usize,
    total: usize,
    config: &Config,
) -> Line<'static> {
    if total == 0 {
        return Line::from(format!("{} {done}/{total}", "░".repeat(PROGRESS_BAR_WIDTH)));
    }
    let mut spans = Vec::new();
    for (status, width) in segment_widths(by_status, total, PROGRESS_BAR_WIDTH) {
        if width == 0 {
            continue;
        }
        spans.push(Span::styled(
            "█".repeat(width),
            config.style_for(Semantic::Glyph(status)),
        ));
    }
    spans.push(Span::raw(format!(" {done}/{total}")));
    Line::from(spans)
}

/// Apportions `width` bar cells across `by_status` counts. Every present
/// status gets at least 1 cell — pure proportional rounding can otherwise
/// erase a status entirely (e.g. 1 wip task out of 15 at width 6 rounds to
/// 0) — then the rest is distributed by largest remainder so segment widths
/// always sum to exactly `width`. Always fits: at most 5 `Status` variants
/// exist and `width` (6) leaves at least 1 cell of slack after the minimums.
/// Iterates in reverse rank order (Done first, Draft last) — `by_status` is
/// a `BTreeMap<Status, _>` sorted Draft..Done, so `.rev()` gives the display
/// order.
fn segment_widths(
    by_status: &BTreeMap<Status, usize>,
    total: usize,
    width: usize,
) -> Vec<(Status, usize)> {
    let mut widths: Vec<(Status, usize)> = by_status.iter().rev().map(|(&s, _)| (s, 1)).collect();
    let leftover = width.saturating_sub(widths.len());
    if leftover > 0 {
        let mut remainders: Vec<(usize, usize)> = Vec::new();
        let mut assigned = 0;
        for i in 0..widths.len() {
            let scaled = by_status[&widths[i].0] * leftover;
            widths[i].1 += scaled / total;
            assigned += scaled / total;
            remainders.push((i, scaled % total));
        }
        let mut deficit = leftover - assigned;
        remainders.sort_by_key(|&(_, rem)| std::cmp::Reverse(rem));
        for (i, _) in remainders {
            if deficit == 0 {
                break;
            }
            widths[i].1 += 1;
            deficit -= 1;
        }
    }
    widths
}

#[cfg(test)]
mod progress_bar_tests {
    use super::*;

    #[test]
    fn segment_widths_sum_to_width() {
        let by_status = BTreeMap::from([(Status::Draft, 1), (Status::Todo, 1), (Status::Done, 1)]);
        let widths = segment_widths(&by_status, 3, 6);
        assert_eq!(widths.iter().map(|&(_, w)| w).sum::<usize>(), 6);
        assert_eq!(
            widths,
            vec![(Status::Done, 2), (Status::Todo, 2), (Status::Draft, 2)]
        );
    }

    #[test]
    fn segment_widths_handles_uneven_split() {
        let by_status = BTreeMap::from([(Status::Draft, 1), (Status::Todo, 2)]);
        let widths = segment_widths(&by_status, 3, 6);
        assert_eq!(widths.iter().map(|&(_, w)| w).sum::<usize>(), 6);
        assert_eq!(widths, vec![(Status::Todo, 4), (Status::Draft, 2)]);
    }

    #[test]
    fn segment_widths_gives_a_rare_status_at_least_one_cell() {
        let by_status = BTreeMap::from([(Status::Draft, 12), (Status::Wip, 1), (Status::Done, 2)]);
        let widths = segment_widths(&by_status, 15, 6);
        assert_eq!(widths.iter().map(|&(_, w)| w).sum::<usize>(), 6);
        assert_eq!(
            widths,
            vec![(Status::Done, 2), (Status::Wip, 1), (Status::Draft, 3)]
        );
    }

    #[test]
    fn progress_bar_empty_subtree_is_all_hollow() {
        let config = Config::load(None).unwrap();
        let line = progress_bar(&BTreeMap::new(), 0, 0, &config);
        assert_eq!(line.to_string(), "░░░░░░ 0/0");
    }
}

fn render_list(f: &mut Frame, area: Rect, app: &AppState, hits: &[todoapp_app::QueryHit]) {
    let inner_width = area.width.saturating_sub(2); // block's left/right border
    let items: Vec<ListItem> = hits
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let breadcrumb = if hit.path.is_empty() {
                String::new()
            } else {
                format!("[{}] ", hit.path.join(" › "))
            };
            let icon = status_icon(hit.task.status, app);
            let prefix = breadcrumb;
            let title_width = usize::from(inner_width)
                .saturating_sub(prefix.chars().count())
                .saturating_sub(icon.chars().count() + 1)
                .max(1);
            let mut spans = vec![
                Span::raw(prefix),
                Span::styled(icon, app.config.style_for(Semantic::Glyph(hit.task.status))),
                Span::raw(" "),
            ];
            let title_base = app.config.style_for(Semantic::Text(hit.task.status));
            let title_spans = match (i == app.cursor, app.selection) {
                (true, Some(sel)) => selection_spans(&hit.task.title, title_width, sel),
                _ => crate::tui::markdown::render_inline(&hit.task.title, title_width),
            };
            spans.extend(
                title_spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, title_base.patch(s.style))),
            );
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" results "))
        .highlight_style(app.config.style_for(Semantic::Selected));
    let mut state = ListState::default().with_selected(Some(app.cursor));
    f.render_widget(Clear, area);
    f.render_stateful_widget(list, area, &mut state);
}

/// Non-modal details pane for the current selection (toggled by
/// `Action::ViewDetail`). Two columns: title + notes (Markdown, scrollable) on
/// the left 2/3, the remaining capabilities as `key: value` lines on the right
/// 1/3. Read-only — the main tree/list keeps focus.
fn render_detail(f: &mut Frame, area: Rect, app: &AppState) {
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" details (v to close · pgup/pgdn scroll) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(pane) = &app.detail else {
        f.render_widget(
            Paragraph::new("no task selected").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    let [left, right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .areas(inner);

    // ---- Left: breadcrumb + title + notes (Markdown), scrollable ----------
    let snap = &pane.snap;
    let mut text = crate::tui::markdown::render(&snap.title);
    if !pane.breadcrumb.is_empty() {
        let crumbs = pane
            .breadcrumb
            .iter()
            .map(|t| short_title(t))
            .collect::<Vec<_>>()
            .join(" / ");
        text.lines.insert(
            0,
            Line::styled(crumbs, Style::default().fg(Color::DarkGray)),
        );
    }
    if let Some(notes) = &snap.notes
        && !notes.is_empty()
    {
        text.lines.push(Line::raw(""));
        text.lines.extend(crate::tui::markdown::render(notes).lines);
    }
    // Reserve the last column of `left` for the scrollbar track.
    let text_area = Rect {
        width: left.width.saturating_sub(1),
        ..left
    };
    let total = u16::try_from(text.lines.len()).unwrap_or(u16::MAX);
    let viewport = text_area.height;
    let max_scroll = total.saturating_sub(viewport);
    let scroll = pane.scroll.min(max_scroll);
    let p = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(p, text_area);
    if total > viewport {
        // ponytail: content length is pre-wrap line count — a hair off once
        // long lines wrap, but the scrollbar only needs to look about right.
        let mut sb_state =
            ScrollbarState::new(usize::from(max_scroll)).position(usize::from(scroll));
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            left,
            &mut sb_state,
        );
    }

    // ---- Right: remaining capabilities as key: value lines ----------------
    f.render_widget(
        Paragraph::new(detail_field_lines(app, pane)).wrap(Wrap { trim: false }),
        right,
    );
}

/// The details pane's right column: one `key: value` line per present
/// capability, omitting absent ones.
fn detail_field_lines(app: &AppState, pane: &DetailPane) -> Vec<Line<'static>> {
    let snap = &pane.snap;
    let label = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = Vec::new();
    let mut field = |k: &str, v: String| {
        lines.push(Line::from(vec![
            Span::styled(format!("{k}: "), label),
            Span::raw(v),
        ]));
    };

    let status = if pane.blocked {
        format!("{} [blocked]", snap.status)
    } else {
        snap.status.to_string()
    };
    field("status", status);
    if let Some(id_disp) = app.short_ids.get(&pane.id) {
        field("id", id_disp.clone());
    }
    if let Some(due) = &snap.due_date {
        field("due", due.to_string());
    }
    if let Some(est) = snap.eta_minutes {
        field(
            "estimate",
            crate::tui::human_duration::format(
                est,
                app.config.hours_per_day,
                app.config.days_per_week,
            ),
        );
    }
    if !snap.time_spent_minutes.0.is_zero() {
        field("spent", snap.time_spent_minutes.to_string());
    }
    if !snap.tags.is_empty() {
        field(
            "tags",
            snap.tags.iter().cloned().collect::<Vec<_>>().join(", "),
        );
    }
    if !snap.assignments.is_empty() {
        let assignees = snap
            .assignments
            .iter()
            .map(|a| {
                if a.claimed {
                    format!("{}*", a.actor.as_str())
                } else {
                    a.actor.as_str().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        field("assignees", assignees);
    }
    match &snap.workspace {
        Some(w) => field("workspace", w.name.clone()),
        None => {
            if let Some(ws) = &pane.inherited_ws {
                field("workspace", format!("{ws} (inherited)"));
            }
        }
    }
    if let Some(r) = &snap.recurrence {
        field("recurs", recurrence_summary(r));
    }
    if let Some(iss) = &snap.issue_ref {
        field("issue", format!("{}#{}", iss.provider, iss.id));
    }
    if !snap.attachments.is_empty() {
        field("attachments", snap.attachments.len().to_string());
    }
    if snap.archived {
        field("archived", "yes".to_string());
    }
    for b in &pane.blockers {
        field("blocked by", b.clone());
    }
    field("created", snap.created_at.0.to_string());
    field("updated", snap.updated_at.0.to_string());

    lines
}

/// A task title's first line, with an ellipsis appended when it has more —
/// used for ancestor crumbs in the details pane (titles can be multi-line).
fn short_title(title: &str) -> String {
    match title.split_once('\n') {
        Some((first, rest)) if !rest.trim().is_empty() => format!("{first}…"),
        Some((first, _)) => first.to_string(),
        None => title.to_string(),
    }
}

/// A compact one-line summary of a recurrence rule for the details pane.
fn recurrence_summary(r: &todoapp_core::Recurrence) -> String {
    use todoapp_core::RepeatCycle;
    match &r.cycle {
        RepeatCycle::Daily { every_n_days: 1 } => "daily".to_string(),
        RepeatCycle::Daily { every_n_days: n } => format!("every {n} days"),
        RepeatCycle::Weekly { weekdays } if weekdays.is_empty() => "weekly".to_string(),
        RepeatCycle::Weekly { weekdays } => {
            let days = weekdays
                .iter()
                .map(|d| format!("{d:?}"))
                .collect::<Vec<_>>()
                .join(",");
            format!("weekly ({days})")
        }
        RepeatCycle::Monthly { every_n_months: 1 } => "monthly".to_string(),
        RepeatCycle::Monthly { every_n_months: n } => format!("every {n} months"),
    }
}

fn render_status_bar(f: &mut Frame, area: Rect, app: &AppState) {
    // A toast (yank confirmation, error, ...) is styled distinctly from the
    // plain keybinding hints so it reads as a transient notice; it auto-hides
    // itself via `status_msg_display` once its TTL elapses.
    let (msg, style) = if let Some(m) = app.status_msg_display() {
        (
            m.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            default_hint(&app.keymap),
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(msg).style(style), area);
}

/// Build the bottom-bar hint from the live keymap (first bound key per action).
fn default_hint(keymap: &Keymap) -> String {
    let k = |a: Action| keymap.keys_for(a).into_iter().next().unwrap_or_default();
    format!(
        "{}/{} nav · {}/{} fold · {} add · {} edit · {} status · {} claim · \
         {}/{} reorder · {}/{} reparent · {} search · {} next · {} help · {} quit",
        k(Action::MoveUp),
        k(Action::MoveDown),
        k(Action::Collapse),
        k(Action::Expand),
        k(Action::AddSibling),
        k(Action::EditTitle),
        k(Action::CycleStatus),
        k(Action::Claim),
        k(Action::ReorderUp),
        k(Action::ReorderDown),
        k(Action::ReparentIn),
        k(Action::ReparentOut),
        k(Action::Search),
        k(Action::WhatNext),
        k(Action::ToggleHelp),
        k(Action::Quit),
    )
}

/// Cap on how tall the add/search dialog grows before it scrolls instead.
const MAX_DIALOG_VISIBLE_ROWS: usize = 10;
/// Cap on how many wrapped rows the edit form's notes field shows before it
/// scrolls instead of growing the popup further.
const MAX_NOTES_VISIBLE_ROWS: usize = 6;

/// The add/child/search text dialog: multi-line, soft-wrapped, growing (up to
/// `MAX_DIALOG_VISIBLE_ROWS`) then vertically scrolling. Enter inserts a
/// newline; Alt+Enter submits (see `app::handle_input_key`).
fn render_confirm_delete_modal(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    id: &todoapp_core::Id,
    has_children: bool,
) {
    let title = app
        .items
        .iter()
        .find(|i| &i.id == id)
        .map_or(id.as_str(), |i| i.title.as_str());
    let text = if has_children {
        format!("Delete '{title}' and all its descendants? (y/N)")
    } else {
        format!("Delete '{title}'? (y/N)")
    };
    let popup = centered_rect(area, 60, 3);
    let p = Paragraph::new(text).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" confirm delete "),
    );
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
}

fn render_input_modal(f: &mut Frame, area: Rect, mode: &InputMode, input: &tui_input::Input) {
    let title = match mode {
        InputMode::AddChild(_) => " new child task (alt+enter submit · esc cancel) ",
        InputMode::AddRoot => " new root task (alt+enter submit · esc cancel) ",
        InputMode::Search => " search (alt+enter submit · esc cancel) ",
        InputMode::Assign(_) => {
            " assign actor(s), comma-separated (alt+enter submit · esc cancel) "
        }
    };
    let width = text_edit::dialog_wrap_width(area.width);
    let rows = text_edit::visual_rows(input, width);
    let visible = rows.len().clamp(1, MAX_DIALOG_VISIBLE_ROWS);
    let popup = centered_rect(area, 60, u16::try_from(visible + 2).unwrap_or(u16::MAX));
    let (cursor_row, cursor_col) = text_edit::cursor_visual_pos(input, width);
    let top = text_edit::viewport_scroll(cursor_row, rows.len(), visible);
    let text = rows[top..(top + visible).min(rows.len())].join("\n");
    let p = Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
    f.set_cursor_position(Position::new(
        popup.x + 1 + u16::try_from(cursor_col).unwrap_or(0),
        popup.y + 1 + u16::try_from(cursor_row.saturating_sub(top)).unwrap_or(0),
    ));
}

/// Wrapped-row layout for one multi-line field: visual rows, how many are
/// shown (capped at `MAX_NOTES_VISIBLE_ROWS`), the top scrolled-to row, and
/// the cursor's visual `(row, col)`.
fn multiline_layout(
    field: &tui_input::Input,
    inner_width: usize,
) -> (Vec<&str>, usize, usize, usize, usize) {
    let rows = text_edit::visual_rows(field, inner_width);
    let visible = rows.len().clamp(1, MAX_NOTES_VISIBLE_ROWS);
    let (cursor_row, cursor_col) = text_edit::cursor_visual_pos(field, inner_width);
    let top = text_edit::viewport_scroll(cursor_row, rows.len(), visible);
    (rows, visible, top, cursor_row, cursor_col)
}

/// The multi-field task edit dialog. Title and notes are multi-line/
/// soft-wrapped like the add dialog, each with its own vertical scroll; the
/// other fields are one line each with their own horizontal scroll.
/// Tab/Shift+Tab cycles focus; the id field is read-only (never gets a
/// cursor).
fn render_edit_form(f: &mut Frame, area: Rect, form: &crate::tui::app::TaskEditForm) {
    let inner_width = text_edit::dialog_wrap_width(area.width);

    let single_line_count = crate::tui::app::EDIT_FORM_LABELS.len() - 2; // all but title/notes
    let multiline_visible_total: usize = [TITLE_FIELD, NOTES_FIELD]
        .iter()
        .map(|&i| multiline_layout(&form.fields[i], inner_width).1)
        .sum();
    let height =
        u16::try_from(single_line_count + 2 + multiline_visible_total + 2).unwrap_or(u16::MAX);
    let popup = centered_rect(area, 60, height);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_pos: Option<(u16, u16)> = None;

    for (i, label) in crate::tui::app::EDIT_FORM_LABELS.iter().enumerate() {
        let focused = i == form.focus;
        if i == TITLE_FIELD || i == NOTES_FIELD {
            let (rows, visible, top, cursor_row, cursor_col) =
                multiline_layout(&form.fields[i], inner_width);
            lines.push(if focused {
                Line::styled(
                    format!("{label}:"),
                    Style::default().add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(format!("{label}:"))
            });
            let visible_rows = &rows[top..(top + visible).min(rows.len())];
            for (row_idx, row) in visible_rows.iter().enumerate() {
                let line_idx = lines.len();
                lines.push(if focused {
                    Line::styled(
                        (*row).to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    )
                } else {
                    Line::raw((*row).to_string())
                });
                if focused && top + row_idx == cursor_row {
                    cursor_pos = Some((
                        u16::try_from(cursor_col).unwrap_or(0),
                        u16::try_from(line_idx).unwrap_or(0),
                    ));
                }
            }
            continue;
        }

        let field = &form.fields[i];
        let avail = inner_width.saturating_sub(label.len() + 2).max(1);
        let value = field.value();
        let total_chars = value.chars().count();
        let scroll = text_edit::viewport_scroll(field.cursor(), total_chars, avail);
        let visible: String = value.chars().skip(scroll).take(avail).collect();
        let text = format!("{label}: {visible}");
        let line_idx = lines.len();
        lines.push(if focused {
            Line::styled(text, Style::default().add_modifier(Modifier::BOLD))
        } else {
            Line::raw(text)
        });
        if focused && i != ID_FIELD {
            let col = label.len() + 2 + (field.cursor() - scroll);
            cursor_pos = Some((
                u16::try_from(col).unwrap_or(0),
                u16::try_from(line_idx).unwrap_or(0),
            ));
        }
    }

    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(
        " edit task (tab/shift+tab fields · notes: enter=newline, alt+enter=save · esc cancel) ",
    ));
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
    if let Some((x, y)) = cursor_pos {
        f.set_cursor_position(Position::new(popup.x + 1 + x, popup.y + 1 + y));
    }
}

/// The workspace-assignment popup opened from the edit form's workspace
/// field: existing workspaces, then inherited/none, then "(new…)".
fn render_ws_picker(f: &mut Frame, area: Rect, picker: &WsPicker) {
    let height = u16::try_from(picker.items.len() + 2).unwrap_or(u16::MAX);
    let popup = centered_rect(area, 50, height);
    let items: Vec<ListItem> = picker
        .items
        .iter()
        .map(|item| ListItem::new(item.label.clone()))
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" assign workspace (enter select · esc cancel) "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(picker.selected));
    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut state);
}

/// The workspace management dialog: one row per known workspace (name /
/// stored default path / this-machine config override), plus a fresh row
/// for defining a new one. The focused cell is highlighted; a cell being
/// typed into shows the in-progress text instead of the row's stored value.
fn render_ws_editor(f: &mut Frame, area: Rect, editor: &WsEditor) {
    let height = u16::try_from(editor.rows.len() + 3).unwrap_or(u16::MAX);
    let popup = centered_rect(area, 80, height);
    let (cur_r, cur_c) = editor.cursor;
    let cell_text = |r: usize, c: usize, stored: &str| -> String {
        if r == cur_r
            && c == cur_c
            && let Some(input) = &editor.editing
        {
            input.value().to_string()
        } else {
            stored.to_string()
        }
    };
    let rows: Vec<Row> = editor
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let name = cell_text(r, 0, if row.is_new { "(new)" } else { &row.name });
            let path = cell_text(r, 1, row.db_path.as_deref().unwrap_or(""));
            let over = cell_text(r, 2, row.override_.as_deref().unwrap_or(""));
            let style = |c: usize| {
                if r == cur_r && c == cur_c {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                }
            };
            Row::new([
                Cell::from(name).style(style(0)),
                Cell::from(path).style(style(1)),
                Cell::from(over).style(style(2)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ],
    )
    .header(Row::new([
        "name",
        "default path (db)",
        "override (this machine)",
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" workspaces (arrows move · enter edit/commit · esc cancel/close) "),
    );
    f.render_widget(Clear, popup);
    f.render_widget(table, popup);
}

/// Title syntax hints (spec `FR-32`/`FR-33`/`FR-34`), appended to the
/// keybinding table since they aren't `Action`s — shown as `(shown text, description)`.
const TITLE_SYNTAX_HINTS: &[(&str, &str)] = &[
    ("", ""),
    ("title syntax:", ""),
    ("@name", "assign"),
    ("#tag", "add tag"),
    ("[date]", "set due (YYYY-MM-DD, HH:mm, weekday, ...)"),
    (
        "[recurrence]",
        "set recurrence (daily, every N days, monthly, ...)",
    ),
];

fn render_help(f: &mut Frame, area: Rect, keymap: &Keymap) {
    let mut rows: Vec<Row> = Action::iter()
        .map(|action| {
            let keys = keymap.keys_for(action).join(" / ");
            Row::new([keys, action.description().to_string()])
        })
        .collect();
    rows.extend(
        TITLE_SYNTAX_HINTS
            .iter()
            .map(|(k, d)| Row::new([(*k).to_string(), (*d).to_string()])),
    );

    let popup = centered_rect(area, 62, u16::try_from(rows.len() + 2).unwrap_or(u16::MAX));
    let table = Table::new(rows, [Constraint::Length(20), Constraint::Fill(1)])
        .block(Block::default().borders(Borders::ALL).title(" help "));
    f.render_widget(Clear, popup);
    f.render_widget(table, popup);
}

/// A centered rectangle: `pct_width` percent of `area` width, fixed `height` rows.
fn centered_rect(area: Rect, pct_width: u16, height: u16) -> Rect {
    let w = area.width * pct_width / 100;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, w, height.min(area.height))
}

/// The configured glyph for a status; `wip` is always the animated spinner,
/// driven by `app.throbber_state` (advanced once per redraw in the event loop).
fn status_icon(s: Status, app: &AppState) -> String {
    if s == Status::Wip {
        let throbber =
            throbber_widgets_tui::Throbber::default().throbber_set(app.config.throbber_set.clone());
        throbber
            .to_symbol_span(&app.throbber_state)
            .content
            .trim()
            .to_string()
    } else {
        app.config
            .status_glyphs
            .get(&s)
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use todoapp_core::{Date, Duration, Id, Status};

    use super::*;
    use crate::svc::make_svc;
    use crate::tui::app::tests::new_app as new_test_app;

    /// Renders `app` to a 130x10 test buffer.
    fn render_to_buffer(app: &AppState) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(130, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    #[tokio::test]
    async fn tree_table_renders_configured_columns_with_eta_overrun_in_red() {
        let mut app = new_test_app().await;

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        let root = svc.create("Root", None, Status::Todo, []).await.unwrap();
        svc.set_estimate(&root.id, Some(Duration::from_minutes(5 * 480)))
            .await
            .unwrap();
        // A due date far in the past guarantees the projected finish overruns it.
        svc.set_due(&root.id, Some(Date::parse("2020-01-01").unwrap().into()))
            .await
            .unwrap();
        svc.assign(&root.id, Id::new("alice")).await.unwrap();
        // A second, unselected row so the cursor highlight (which overrides fg)
        // doesn't mask the red eta styling under test.
        svc.create("Other", None, Status::Todo, []).await.unwrap();
        app.rebuild().await;
        app.cursor = app.items.iter().position(|i| i.title == "Other").unwrap();

        let buf = render_to_buffer(&app);
        let rendered = buf
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(rendered.contains("Root"));
        assert!(rendered.contains("due"));
        assert!(rendered.contains("eta"));
        assert!(rendered.contains("assignee"));
        assert!(rendered.contains("alice"));
        assert!(rendered.contains("2020-01-01"));

        // The eta column cell must be styled red (projection overruns the due date).
        let has_red_cell =
            (0..buf.area.width).any(|x| (0..buf.area.height).any(|y| buf[(x, y)].fg == Color::Red));
        assert!(
            has_red_cell,
            "expected a red-styled eta cell for the overrun"
        );
    }

    #[tokio::test]
    async fn help_popup_documents_title_syntax() {
        let mut app = new_test_app().await;
        app.view = crate::tui::app::View::Help;

        // Tall enough to fit every keybinding row plus the title-syntax hints
        // appended after them (the popup's height is capped to the terminal's).
        let backend = TestBackend::new(130, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let rendered = buf
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(rendered.contains("@name"));
        assert!(rendered.contains("#tag"));
        assert!(rendered.contains("[date]"));
        assert!(rendered.contains("[recurrence]"));
    }

    #[tokio::test]
    async fn select_mode_highlights_the_selected_range_with_reversed_style() {
        let mut app = new_test_app().await;

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        svc.create("Hello world", None, Status::Todo, [])
            .await
            .unwrap();
        app.rebuild().await;
        app.cursor = 0;
        app.selection = Some(crate::tui::app::Selection {
            anchor: 0,
            cursor: 4,
        });

        let buf = render_to_buffer(&app);
        let has_reversed_cell = (0..buf.area.width).any(|x| {
            (0..buf.area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED))
        });
        assert!(
            has_reversed_cell,
            "expected a reversed-style cell for the active selection range"
        );
    }

    #[tokio::test]
    async fn aggregate_row_dims_worst_case_status_and_leaf_glyph_differs_from_text() {
        let mut app = new_test_app().await;

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        let root = svc.create("Root", None, Status::Todo, []).await.unwrap();
        svc.create("Done child", Some(&root.id), Status::Done, [])
            .await
            .unwrap();
        svc.create("Todo child", Some(&root.id), Status::Todo, [])
            .await
            .unwrap();
        app.rebuild().await;
        app.expanded.insert(root.id.clone());
        app.rebuild().await;
        // Cursor on "Todo child" so the selection highlight (which overrides
        // fg/modifier) doesn't mask the Root/Done-child styling under test.
        app.cursor = app
            .items
            .iter()
            .position(|i| i.title == "Todo child")
            .unwrap();

        let buf = render_to_buffer(&app);
        let row_text =
            |y: u16| -> String { (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect() };

        // Root rolls up to Todo (worst-case among Done/Todo children); Todo's
        // text style is plain, but the aggregate modifier (DIM) must still
        // show up on the parent row.
        let root_y = (0..buf.area.height)
            .find(|&y| row_text(y).contains("Root"))
            .expect("Root row must be rendered");
        let root_row_dimmed =
            (0..buf.area.width).any(|x| buf[(x, root_y)].modifier.contains(Modifier::DIM));
        assert!(
            root_row_dimmed,
            "expected the aggregate row's title to carry the DIM modifier"
        );
        // ...but only the title, not the whole row: the rightmost content
        // column (the "id" column, unrelated to status) must stay plain.
        let last_col_x = buf.area.width - 2; // inside the block's right border
        assert!(
            !buf[(last_col_x, root_y)].modifier.contains(Modifier::DIM),
            "expected only the title to be dimmed, not the whole row"
        );

        // The Done leaf's glyph (✔) is green while its title text is gray +
        // crossed-out — two different cells, not one shared color.
        let done_y = (0..buf.area.height)
            .find(|&y| row_text(y).contains("Done child"))
            .expect("Done child row must be rendered");
        let glyph_x = (0..buf.area.width)
            .find(|&x| buf[(x, done_y)].symbol() == "✔")
            .expect("glyph cell must be rendered");
        assert_eq!(buf[(glyph_x, done_y)].fg, Color::Green);
        let title_x = (0..buf.area.width)
            .find(|&x| buf[(x, done_y)].symbol() == "D" && x > glyph_x)
            .expect("title cell must be rendered");
        assert_eq!(buf[(title_x, done_y)].fg, Color::DarkGray);
        assert!(
            buf[(title_x, done_y)]
                .modifier
                .contains(Modifier::CROSSED_OUT)
        );

        // The progress bar's filled run uses the same color as the Done glyph.
        let has_green_bar_cell = (0..buf.area.width)
            .any(|x| (0..buf.area.height).any(|y| buf[(x, y)].fg == Color::Green));
        assert!(
            has_green_bar_cell,
            "expected a green-filled progress bar cell"
        );
    }
}
