//! Pure rendering: takes `&AppState`, draws to `Frame`. No mutations.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Wrap,
    },
};
use todoapp_core::Status;

use crate::app::{
    AppState, ID_FIELD, InputMode, NOTES_FIELD, Selection, TITLE_FIELD, View, VisibleItem,
};
use crate::config::ColumnKind;
use crate::keymap::{Action, Keymap};
use crate::text_edit;

pub fn render(f: &mut Frame, app: &AppState) {
    let area = f.area();
    let [main, status_bar] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);

    if let View::Detail { title, notes } = &app.view {
        // Tree keeps the top half so the cursor's context stays visible;
        // the detail pane takes the bottom half rather than covering it.
        let [tree_area, detail_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(main);
        render_tree(f, tree_area, app);
        render_detail(f, detail_area, title, notes);
    } else {
        render_tree(f, main, app);
        if let View::List(hits) = &app.view {
            render_list(f, main, app, hits);
        }
    }

    render_status_bar(f, status_bar, app);

    if let Some((mode, input)) = &app.input {
        render_input_modal(f, area, mode, input);
    }
    if let Some(form) = &app.edit_form {
        render_edit_form(f, area, form);
    }
    if matches!(app.view, View::Help) {
        render_help(f, area, &app.keymap);
    }
}

/// The tree column (indent + expand arrow + status icon + title + blocked
/// badge) plus one configured column per capability (spec: values are always
/// the aggregate over the task + its descendants, `Services::aggregate`).
fn render_tree(f: &mut Frame, area: Rect, app: &AppState) {
    let columns = &app.config.columns;
    let header = Row::new(
        std::iter::once(Cell::from("tree")).chain(columns.iter().map(|c| Cell::from(c.header()))),
    );
    // Approximate the tree (title) column's rendered width: total width minus
    // the block's left/right border, the other configured columns, and one
    // column-spacing gap per configured column (Table's default spacing).
    // ponytail: an approximation, not ratatui's exact layout math — only
    // used to size the ellipsis truncation, a char or two off doesn't matter.
    let configured_width: u16 = columns.iter().map(|c| column_width(*c)).sum();
    let gaps = u16::try_from(columns.len()).unwrap_or(0);
    let tree_col_width = area.width.saturating_sub(2 + configured_width + gaps);
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
        .row_highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = TableState::default().with_selected(Some(app.cursor));
    f.render_stateful_widget(table, area, &mut state);
}

fn column_width(kind: ColumnKind) -> u16 {
    match kind {
        ColumnKind::Status | ColumnKind::Due | ColumnKind::Eta => 10,
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
    let tree_cell = Cell::from(tree_cell_line(item, app, tree_col_width, is_cursor))
        .style(tree_status_style(item.status));
    let cells =
        std::iter::once(tree_cell).chain(columns.iter().map(|c| render_column(item, *c, app)));
    Row::new(cells)
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
    let prefix = format!("{indent}{arrow}{icon} ");
    let badge_width = if item.is_blocked { 4 } else { 0 }; // " [!]"
    let title_width = usize::from(tree_col_width)
        .saturating_sub(prefix.chars().count())
        .saturating_sub(badge_width)
        .max(1);
    let mut spans = vec![Span::raw(prefix)];
    match (is_cursor, app.selection) {
        (true, Some(sel)) => spans.extend(selection_spans(&item.title, title_width, sel)),
        _ => spans.extend(crate::markdown::render_inline(&item.title, title_width)),
    }
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
        ColumnKind::Status => Cell::from(progress_bar(item.done, item.total)),
        ColumnKind::Due => Cell::from(item.due.map_or("-".to_string(), |d| d.to_string())),
        ColumnKind::Eta => match item.eta {
            Some((date, overrun)) => {
                let cell = Cell::from(date.to_string());
                if overrun {
                    cell.style(Style::default().fg(Color::Red))
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
        ColumnKind::Estimate => Cell::from(crate::human_duration::format(
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

/// A small `[####------] d/t` progress bar, status-count based (ignores estimate).
fn progress_bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 6;
    let filled = (done * WIDTH).checked_div(total).unwrap_or(0);
    let bar: String = "█".repeat(filled) + &"░".repeat(WIDTH - filled);
    format!("{bar} {done}/{total}")
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
            let prefix = format!("{breadcrumb}{icon} ");
            let title_width = usize::from(inner_width)
                .saturating_sub(prefix.chars().count())
                .max(1);
            let mut spans = vec![Span::raw(prefix)];
            match (i == app.cursor, app.selection) {
                (true, Some(sel)) => {
                    spans.extend(selection_spans(&hit.task.title, title_width, sel));
                }
                _ => spans.extend(crate::markdown::render_inline(&hit.task.title, title_width)),
            }
            ListItem::new(Line::from(spans)).style(status_style(hit.task.status))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" results "))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(app.cursor));
    f.render_widget(Clear, area);
    f.render_stateful_widget(list, area, &mut state);
}

/// Read-only rendered view of a task's title + notes (Markdown), opened by
/// `Action::ViewDetail`. `q`/`esc` (the generic `Action::Quit` back-out)
/// returns to the tree.
fn render_detail(f: &mut Frame, area: Rect, title: &str, notes: &str) {
    let mut text = crate::markdown::render(title);
    if !notes.is_empty() {
        text.lines.push(Line::raw(""));
        text.lines.extend(crate::markdown::render(notes).lines);
    }
    let p = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" details (q/esc to close) "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(Clear, area);
    f.render_widget(p, area);
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
fn render_input_modal(f: &mut Frame, area: Rect, mode: &InputMode, input: &tui_input::Input) {
    let title = match mode {
        InputMode::AddChild(_) => " new child task (alt+enter submit · esc cancel) ",
        InputMode::AddRoot => " new root task (alt+enter submit · esc cancel) ",
        InputMode::Search => " search (alt+enter submit · esc cancel) ",
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
fn render_edit_form(f: &mut Frame, area: Rect, form: &crate::app::TaskEditForm) {
    let inner_width = text_edit::dialog_wrap_width(area.width);

    let single_line_count = crate::app::EDIT_FORM_LABELS.len() - 2; // all but title/notes
    let multiline_visible_total: usize = [TITLE_FIELD, NOTES_FIELD]
        .iter()
        .map(|&i| multiline_layout(&form.fields[i], inner_width).1)
        .sum();
    let height =
        u16::try_from(single_line_count + 2 + multiline_visible_total + 2).unwrap_or(u16::MAX);
    let popup = centered_rect(area, 60, height);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_pos: Option<(u16, u16)> = None;

    for (i, label) in crate::app::EDIT_FORM_LABELS.iter().enumerate() {
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

fn render_help(f: &mut Frame, area: Rect, keymap: &Keymap) {
    let rows: Vec<Row> = Action::iter()
        .map(|action| {
            let keys = keymap.keys_for(action).join(" / ");
            Row::new([keys, action.description().to_string()])
        })
        .collect();

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

/// Tree-column style: gray for `done` tasks, default text color otherwise.
fn tree_status_style(s: Status) -> Style {
    match s {
        Status::Done => Style::default().fg(Color::Gray),
        Status::Draft | Status::Todo | Status::Wip | Status::Paused => Style::default(),
    }
}

fn status_style(s: Status) -> Style {
    match s {
        Status::Draft => Style::default().fg(Color::DarkGray),
        Status::Todo => Style::default(),
        Status::Wip => Style::default().fg(Color::Yellow),
        Status::Paused => Style::default().fg(Color::Cyan),
        Status::Done => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::DIM),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use todoapp_core::{Date, Duration, Id, Status};

    use super::*;
    use crate::app::make_svc;
    use crate::app::tests::new_app as new_test_app;

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
    async fn select_mode_highlights_the_selected_range_with_reversed_style() {
        let mut app = new_test_app().await;

        let svc = make_svc(&app.store, &app.clock, &app.ids);
        svc.create("Hello world", None, Status::Todo, [])
            .await
            .unwrap();
        app.rebuild().await;
        app.cursor = 0;
        app.selection = Some(crate::app::Selection {
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
}
