//! Pure rendering: takes `&AppState`, draws to `Frame`. No mutations.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
};
use tda_core::Status;

use crate::app::{AppState, InputMode, View, VisibleItem};
use crate::config::ColumnKind;
use crate::keymap::{Action, Keymap};

pub fn render(f: &mut Frame, app: &AppState) {
    let area = f.area();
    let [main, status_bar] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);

    // Draw the tree table behind any overlay.
    render_tree(f, main, app);

    if let View::List(hits) = &app.view {
        render_list(f, main, app, hits);
    }

    render_status_bar(f, status_bar, app);

    if let Some((mode, text)) = &app.input {
        render_input_modal(f, area, mode, text);
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
    let rows: Vec<Row> = app
        .items
        .iter()
        .map(|item| item_row(item, columns, app))
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

fn item_row(item: &VisibleItem, columns: &[ColumnKind], app: &AppState) -> Row<'static> {
    let tree_cell = Cell::from(tree_cell_text(item, app)).style(tree_status_style(item.status));
    let cells =
        std::iter::once(tree_cell).chain(columns.iter().map(|c| render_column(item, *c, app)));
    Row::new(cells)
}

fn tree_cell_text(item: &VisibleItem, app: &AppState) -> String {
    let indent = "  ".repeat(item.depth);
    let arrow = if item.has_children {
        if item.is_expanded { "▼ " } else { "▶ " }
    } else {
        "· "
    };
    let badge = if item.is_blocked { " [!]" } else { "" };
    let icon = status_icon(item.status, app);
    format!("{indent}{arrow}{icon} {}{badge}", item.title)
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

fn render_list(f: &mut Frame, area: Rect, app: &AppState, hits: &[tda_app::QueryHit]) {
    let items: Vec<ListItem> = hits
        .iter()
        .map(|hit| {
            let breadcrumb = if hit.path.is_empty() {
                String::new()
            } else {
                format!("[{}] ", hit.path.join(" › "))
            };
            let icon = status_icon(hit.task.status, app);
            let content = format!("{breadcrumb}{icon} {}", hit.task.title);
            ListItem::new(content).style(status_style(hit.task.status))
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

fn render_status_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let hint;
    let msg: &str = if let Some(m) = &app.status_msg {
        m.as_str()
    } else {
        hint = default_hint(&app.keymap);
        &hint
    };
    f.render_widget(
        Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)),
        area,
    );
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

fn render_input_modal(f: &mut Frame, area: Rect, mode: &InputMode, text: &str) {
    let title = match mode {
        InputMode::AddChild(_) => " new child task ",
        InputMode::AddRoot => " new root task ",
        InputMode::Search => " search ",
    };
    let popup = centered_rect(area, 60, 3);
    let p = Paragraph::new(format!("{text}▌"))
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
}

/// The multi-field task edit dialog: one line per field, the focused field
/// gets a cursor and a highlight style (Tab/Shift+Tab cycles focus).
fn render_edit_form(f: &mut Frame, area: Rect, form: &crate::app::TaskEditForm) {
    use ratatui::text::Line;

    let height = u16::try_from(crate::app::EDIT_FORM_LABELS.len() + 2).unwrap_or(u16::MAX);
    let popup = centered_rect(area, 60, height);
    let lines: Vec<Line> = crate::app::EDIT_FORM_LABELS
        .iter()
        .zip(&form.fields)
        .enumerate()
        .map(|(i, (label, value))| {
            let focused = i == form.focus;
            let cursor = if focused { "▌" } else { "" };
            let text = format!("{label}: {value}{cursor}");
            if focused {
                Line::styled(text, Style::default().add_modifier(Modifier::BOLD))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    let p =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" edit task "));
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
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
    use tda_app::Services;
    use tda_core::{Date, Duration, Id, Status};
    use tda_store_turso::TursoStore;

    use super::*;
    use crate::app::AppState;
    use crate::config::Config;
    use crate::keymap::Keymap;

    #[tokio::test]
    async fn tree_table_renders_configured_columns_with_eta_overrun_in_red() {
        let mut app = AppState::new(
            TursoStore::open_memory().await,
            Keymap::load(None).unwrap(),
            Config::load(None).unwrap(),
        )
        .await
        .unwrap();

        let svc = Services {
            store: &app.store,
            links: &app.store,
            collections: &app.store,
            query: &app.store,
            clock: &app.clock,
            ids: &app.ids,
        };
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

        let backend = TestBackend::new(130, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
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
        let buf = terminal.backend().buffer();
        let has_red_cell =
            (0..buf.area.width).any(|x| (0..buf.area.height).any(|y| buf[(x, y)].fg == Color::Red));
        assert!(
            has_red_cell,
            "expected a red-styled eta cell for the overrun"
        );
    }
}
