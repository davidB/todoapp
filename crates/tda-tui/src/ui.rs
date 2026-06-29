//! Pure rendering: takes `&AppState`, draws to `Frame`. No mutations.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Row, Table},
};
use tda_core::Status;

use crate::app::{AppState, InputMode, View, VisibleItem};

pub fn render(f: &mut Frame, app: &AppState) {
    let area = f.area();
    let [main, status_bar] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);

    // Draw the tree behind any overlay.
    render_tree(f, main, app);

    if let View::List(hits) = &app.view {
        render_list(f, main, app, hits);
    }

    render_status_bar(f, status_bar, app);

    if let Some((mode, text)) = &app.input {
        render_input_modal(f, area, mode, text);
    }
    if matches!(app.view, View::Help) {
        render_help(f, area);
    }
}

fn render_tree(f: &mut Frame, area: Rect, app: &AppState) {
    let items: Vec<ListItem> = app.items.iter().map(item_line).collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" tda "))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(app.cursor));
    f.render_stateful_widget(list, area, &mut state);
}

fn item_line(item: &VisibleItem) -> ListItem<'static> {
    let indent = "  ".repeat(item.depth);
    let arrow = if item.has_children {
        if item.is_expanded { "▼ " } else { "▶ " }
    } else {
        "· "
    };
    let badge = if item.is_blocked { " [!]" } else { "" };
    let icon = status_icon(item.status);
    let content = format!("{indent}{arrow}[{icon}] {}{badge}", item.title);
    ListItem::new(content).style(status_style(item.status))
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
            let icon = status_icon(hit.task.status);
            let content = format!("{breadcrumb}[{icon}] {}", hit.task.title);
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
    let msg = app.status_msg.as_deref().unwrap_or(
        "j/k↑↓ nav · h/l←→ fold · a add · e edit · Space status · c claim · J/K reorder · / search · n next · ? help · q quit",
    );
    f.render_widget(
        Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_input_modal(f: &mut Frame, area: Rect, mode: &InputMode, text: &str) {
    let title = match mode {
        InputMode::AddChild(_) => " new child task ",
        InputMode::AddRoot => " new root task ",
        InputMode::EditTitle(_) => " edit title ",
        InputMode::Search => " search ",
    };
    let popup = centered_rect(area, 60, 3);
    let p = Paragraph::new(format!("{text}▌"))
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(Clear, popup);
    f.render_widget(p, popup);
}

fn render_help(f: &mut Frame, area: Rect) {
    let rows: Vec<Row> = [
        ("j / ↓", "move down"),
        ("k / ↑", "move up"),
        ("h / ←", "collapse / jump to parent"),
        ("l / → / Enter", "expand"),
        ("g / Home", "first item"),
        ("G / End", "last item"),
        ("a", "add child under cursor"),
        ("A", "add root task"),
        ("e", "edit title"),
        ("Space", "cycle status draft→todo→wip→done"),
        ("c", "claim (→ wip, single-user 'me')"),
        ("J / K", "reorder down / up among siblings"),
        ("/", "text search"),
        ("n", "what-next (status:todo by priority)"),
        ("? / Esc / q", "help / back / quit"),
    ]
    .iter()
    .map(|(k, v)| Row::new([*k, *v]))
    .collect();

    let popup = centered_rect(area, 62, 19);
    let table = Table::new(rows, [Constraint::Length(18), Constraint::Fill(1)])
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

fn status_icon(s: Status) -> char {
    match s {
        Status::Draft => '-',
        Status::Todo => ' ',
        Status::Wip => '~',
        Status::Done => 'x',
    }
}

fn status_style(s: Status) -> Style {
    match s {
        Status::Draft => Style::default().fg(Color::DarkGray),
        Status::Todo => Style::default(),
        Status::Wip => Style::default().fg(Color::Yellow),
        Status::Done => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::DIM),
    }
}
