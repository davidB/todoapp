//! General TUI config: tree-table column order/visibility + the work
//! calendar used to project the `eta` column. Same embedded-default +
//! user-override pattern as [`crate::tui::keymap`]: an optional user file at
//! `~/.config/tda/tui.toml` (path + generic TOML parsing via
//! `todoapp_config::{tui_config_path, read_toml}`) overrides individual
//! fields; unmentioned fields keep their embedded defaults. Shares the file
//! with `[keybindings]` (see [`crate::tui::keymap`]) — this module only reads the
//! `columns`/`schedule`/`status`/`styles`/`behavior` tables and ignores the
//! rest.

use std::collections::BTreeMap;

use anyhow::{Context as _, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use todoapp_core::{Duration, Status};

const DEFAULT_CONFIG_TOML: &str = include_str!("../tui.default.toml");

/// (config name, Status) — single source of truth for the TUI's status naming.
const STATUS_NAMES: &[(&str, Status)] = &[
    ("draft", Status::Draft),
    ("todo", Status::Todo),
    ("wip", Status::Wip),
    ("paused", Status::Paused),
    ("done", Status::Done),
];

fn status_from_name(name: &str) -> Option<Status> {
    STATUS_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}

/// (config name, symbol set) — the throbber sets a user may pick via `status.spinner_set`.
const THROBBER_SETS: &[(&str, throbber_widgets_tui::Set)] = &[
    ("ascii", throbber_widgets_tui::ASCII),
    ("box_drawing", throbber_widgets_tui::BOX_DRAWING),
    ("braille_one", throbber_widgets_tui::BRAILLE_ONE),
    ("braille_double", throbber_widgets_tui::BRAILLE_DOUBLE),
    ("braille_six", throbber_widgets_tui::BRAILLE_SIX),
    (
        "braille_six_double",
        throbber_widgets_tui::BRAILLE_SIX_DOUBLE,
    ),
    ("braille_eight", throbber_widgets_tui::BRAILLE_EIGHT),
    (
        "braille_eight_double",
        throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE,
    ),
];

fn throbber_set_from_name(name: &str) -> Option<throbber_widgets_tui::Set> {
    THROBBER_SETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s.clone())
}

/// (config name, `Modifier`) — single source of truth for style-string modifier names.
const MODIFIER_NAMES: &[(&str, Modifier)] = &[
    ("bold", Modifier::BOLD),
    ("dim", Modifier::DIM),
    ("italic", Modifier::ITALIC),
    ("underlined", Modifier::UNDERLINED),
    ("crossed_out", Modifier::CROSSED_OUT),
    ("reversed", Modifier::REVERSED),
];

fn modifier_from_name(name: &str) -> Option<Modifier> {
    MODIFIER_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| *m)
}

/// Parses `"color"` or `"color,modifier[,modifier...]"` into a `Style` — the
/// one string format the `[styles]` table uses throughout.
fn parse_style(s: &str) -> anyhow::Result<Style> {
    let mut parts = s.split(',').map(str::trim);
    let color = parts.next().unwrap_or("reset");
    let mut style = Style::default().fg(color
        .parse()
        .with_context(|| format!("unknown color {color:?}"))?);
    for m in parts {
        style = style.add_modifier(
            modifier_from_name(m).with_context(|| format!("unknown modifier {m:?}"))?,
        );
    }
    Ok(style)
}

/// What a color/style in `[styles]` is being applied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantic {
    /// The status icon's color only.
    Glyph(Status),
    /// Row/title text style for a leaf task at this status.
    Text(Status),
    /// `Text(s)` with `aggregate_modifier` layered on top — a row that
    /// summarizes a subtree (has children), not a single leaf task.
    AggregateText(Status),
    /// The eta cell when its projected finish overruns the due date.
    Overdue,
    /// The cursor/selected row (tree and list views).
    Selected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Status,
    Due,
    Eta,
    Assignee,
    Estimate,
    Elapsed,
    Tags,
    Id,
}

impl ColumnKind {
    /// (config name, variant, column header) — single source of truth.
    const ALL: &'static [(&'static str, ColumnKind, &'static str)] = &[
        ("status", ColumnKind::Status, "status"),
        ("due", ColumnKind::Due, "due"),
        ("eta", ColumnKind::Eta, "eta"),
        ("assignee", ColumnKind::Assignee, "assignee"),
        ("estimate", ColumnKind::Estimate, "estimate"),
        ("elapsed", ColumnKind::Elapsed, "elapsed"),
        ("tags", ColumnKind::Tags, "tags"),
        ("id", ColumnKind::Id, "id"),
    ];

    fn from_name(name: &str) -> Option<ColumnKind> {
        Self::ALL
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, c, _)| *c)
    }

    pub fn header(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, c, _)| *c == self)
            .map_or("", |(_, _, h)| h)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawColumns {
    order: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawSchedule {
    hours_per_day: Option<f64>,
    days_per_week: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
struct RawStatus {
    enabled: Option<Vec<String>>,
    glyphs: Option<BTreeMap<String, String>>,
    spinner_set: Option<String>,
    spinner_interval_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct RawStyles {
    glyph_colors: Option<BTreeMap<String, String>>,
    text_styles: Option<BTreeMap<String, String>>,
    aggregate_modifier: Option<String>,
    overdue_color: Option<String>,
    selected_bg: Option<String>,
    selected_fg: Option<String>,
    selected_modifier: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawBehavior {
    chain_add: Option<bool>,
    submit_on_enter: Option<bool>,
    restore_state: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    columns: RawColumns,
    #[serde(default)]
    schedule: RawSchedule,
    #[serde(default)]
    status: RawStatus,
    #[serde(default)]
    styles: RawStyles,
    #[serde(default)]
    behavior: RawBehavior,
}

pub struct Config {
    pub columns: Vec<ColumnKind>,
    pub hours_per_day: Duration,
    pub days_per_week: u8,
    /// Enabled statuses, in cycle order (`space` in the TUI walks this list).
    pub status_order: Vec<Status>,
    /// Static glyph per status, except `Wip` (always spinner-driven).
    pub status_glyphs: BTreeMap<Status, String>,
    pub throbber_set: throbber_widgets_tui::Set,
    pub spinner_interval: std::time::Duration,
    /// Fg color for the status glyph/icon only — every status has one.
    pub glyph_colors: BTreeMap<Status, Color>,
    /// Sparse per-status text style override; a status absent here renders
    /// with `Style::default()` (plain — only the glyph carries its color).
    pub text_styles: BTreeMap<Status, Style>,
    /// Layered on top of a `Text` style for rows that summarize a subtree.
    pub aggregate_modifier: Modifier,
    pub overdue_style: Style,
    pub selected_style: Style,
    /// Keep the add-task dialog open after Alt+Enter for rapid batch entry.
    pub chain_add: bool,
    /// In the add/input dialog, plain Enter submits and Shift+Enter inserts a
    /// newline (requires a terminal with the keyboard-enhancement protocol).
    /// When false, the default holds: Alt+Enter submits, plain Enter = newline.
    pub submit_on_enter: bool,
    /// Reload the last session's tree expansion, cursor, and details-pane
    /// visibility on launch, and save them on exit (`tui.state.json` next to
    /// the db). On by default.
    pub restore_state: bool,
}

impl Config {
    /// Resolves a [`Semantic`] to the `Style` it should render with.
    pub fn style_for(&self, semantic: Semantic) -> Style {
        match semantic {
            Semantic::Glyph(s) => {
                Style::default().fg(self.glyph_colors.get(&s).copied().unwrap_or(Color::Reset))
            }
            Semantic::Text(s) => self.text_styles.get(&s).copied().unwrap_or_default(),
            Semantic::AggregateText(s) => self
                .style_for(Semantic::Text(s))
                .add_modifier(self.aggregate_modifier),
            Semantic::Overdue => self.overdue_style,
            Semantic::Selected => self.selected_style,
        }
    }
}

impl Config {
    /// Load defaults, then apply `user` overrides (if given) on top — each
    /// present field replaces its default; unmentioned fields keep their
    /// embedded default. `user` is the already-parsed `[columns]`/
    /// `[schedule]`/`[status]`/`[styles]`/`[behavior]` tables of `tui.toml`
    /// (see `todoapp_config::read_toml`); this module only reads those and
    /// ignores `[keybindings]` (see [`crate::tui::keymap`]).
    #[allow(clippy::too_many_lines, clippy::similar_names)]
    pub fn load(user: Option<&toml::Value>) -> anyhow::Result<Self> {
        let default: RawConfig =
            toml::from_str(DEFAULT_CONFIG_TOML).context("parse embedded default config")?;
        let mut columns = default
            .columns
            .order
            .context("default config missing columns.order")?;
        let mut hours_per_day = default
            .schedule
            .hours_per_day
            .context("default config missing schedule.hours_per_day")?;
        let mut days_per_week = default
            .schedule
            .days_per_week
            .context("default config missing schedule.days_per_week")?;
        let mut status_enabled = default
            .status
            .enabled
            .context("default config missing status.enabled")?;
        let mut status_glyphs = default
            .status
            .glyphs
            .context("default config missing status.glyphs")?;
        let mut spinner_set_name = default
            .status
            .spinner_set
            .context("default config missing status.spinner_set")?;
        let mut spinner_interval_ms = default
            .status
            .spinner_interval_ms
            .context("default config missing status.spinner_interval_ms")?;
        let mut glyph_colors = default
            .styles
            .glyph_colors
            .context("default config missing styles.glyph_colors")?;
        let mut text_styles = default.styles.text_styles.unwrap_or_default();
        let mut aggregate_modifier_name = default
            .styles
            .aggregate_modifier
            .context("default config missing styles.aggregate_modifier")?;
        let mut overdue_color = default
            .styles
            .overdue_color
            .context("default config missing styles.overdue_color")?;
        let mut selected_bg = default
            .styles
            .selected_bg
            .context("default config missing styles.selected_bg")?;
        let mut selected_fg = default
            .styles
            .selected_fg
            .context("default config missing styles.selected_fg")?;
        let mut selected_modifier_name = default
            .styles
            .selected_modifier
            .context("default config missing styles.selected_modifier")?;
        let mut chain_add = default
            .behavior
            .chain_add
            .context("default config missing behavior.chain_add")?;
        let mut submit_on_enter = default
            .behavior
            .submit_on_enter
            .context("default config missing behavior.submit_on_enter")?;
        let mut restore_state = default
            .behavior
            .restore_state
            .context("default config missing behavior.restore_state")?;

        if let Some(user) = user {
            let overrides = RawConfig::deserialize(user.clone()).context("parse user config")?;
            if let Some(order) = overrides.columns.order {
                columns = order;
            }
            if let Some(h) = overrides.schedule.hours_per_day {
                hours_per_day = h;
            }
            if let Some(d) = overrides.schedule.days_per_week {
                days_per_week = d;
            }
            if let Some(enabled) = overrides.status.enabled {
                status_enabled = enabled;
            }
            if let Some(glyphs) = overrides.status.glyphs {
                status_glyphs = glyphs;
            }
            if let Some(set) = overrides.status.spinner_set {
                spinner_set_name = set;
            }
            if let Some(ms) = overrides.status.spinner_interval_ms {
                spinner_interval_ms = ms;
            }
            if let Some(colors) = overrides.styles.glyph_colors {
                glyph_colors = colors;
            }
            if let Some(styles) = overrides.styles.text_styles {
                text_styles = styles;
            }
            if let Some(m) = overrides.styles.aggregate_modifier {
                aggregate_modifier_name = m;
            }
            if let Some(c) = overrides.styles.overdue_color {
                overdue_color = c;
            }
            if let Some(c) = overrides.styles.selected_bg {
                selected_bg = c;
            }
            if let Some(c) = overrides.styles.selected_fg {
                selected_fg = c;
            }
            if let Some(m) = overrides.styles.selected_modifier {
                selected_modifier_name = m;
            }
            if let Some(v) = overrides.behavior.chain_add {
                chain_add = v;
            }
            if let Some(v) = overrides.behavior.submit_on_enter {
                submit_on_enter = v;
            }
            if let Some(v) = overrides.behavior.restore_state {
                restore_state = v;
            }
        }

        let columns = columns
            .iter()
            .map(|name| {
                ColumnKind::from_name(name).with_context(|| format!("unknown column {name:?}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if hours_per_day <= 0.0 {
            bail!("schedule.hours_per_day must be positive, got {hours_per_day}");
        }
        if status_enabled.is_empty() {
            bail!("status.enabled must not be empty");
        }
        let status_order = status_enabled
            .iter()
            .map(|name| status_from_name(name).with_context(|| format!("unknown status {name:?}")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let status_glyphs = status_glyphs
            .into_iter()
            .map(|(name, glyph)| {
                status_from_name(&name)
                    .with_context(|| format!("unknown status {name:?}"))
                    .map(|s| (s, glyph))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let throbber_set = throbber_set_from_name(&spinner_set_name)
            .with_context(|| format!("unknown status.spinner_set {spinner_set_name:?}"))?;
        let glyph_colors = glyph_colors
            .into_iter()
            .map(|(name, color)| {
                let status =
                    status_from_name(&name).with_context(|| format!("unknown status {name:?}"))?;
                let color: Color = color
                    .parse()
                    .with_context(|| format!("unknown color {color:?}"))?;
                Ok((status, color))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let text_styles = text_styles
            .into_iter()
            .map(|(name, style)| {
                let status =
                    status_from_name(&name).with_context(|| format!("unknown status {name:?}"))?;
                Ok((status, parse_style(&style)?))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let aggregate_modifier =
            modifier_from_name(&aggregate_modifier_name).with_context(|| {
                format!("unknown styles.aggregate_modifier {aggregate_modifier_name:?}")
            })?;
        let overdue_style = Style::default().fg(overdue_color
            .parse()
            .with_context(|| format!("unknown color {overdue_color:?}"))?);
        let selected_style = Style::default()
            .bg(selected_bg
                .parse()
                .with_context(|| format!("unknown color {selected_bg:?}"))?)
            .fg(selected_fg
                .parse()
                .with_context(|| format!("unknown color {selected_fg:?}"))?)
            .add_modifier(
                modifier_from_name(&selected_modifier_name).with_context(|| {
                    format!("unknown styles.selected_modifier {selected_modifier_name:?}")
                })?,
            );

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let minutes = (hours_per_day * 60.0).round() as u32;
        Ok(Self {
            columns,
            hours_per_day: Duration::from_minutes(minutes),
            days_per_week,
            status_order,
            status_glyphs,
            throbber_set,
            spinner_interval: std::time::Duration::from_millis(spinner_interval_ms),
            glyph_colors,
            text_styles,
            aggregate_modifier,
            overdue_style,
            selected_style,
            chain_add,
            submit_on_enter,
            restore_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> toml::Value {
        toml::from_str(s).expect("valid TOML")
    }

    #[test]
    fn default_config_parses() {
        let config = Config::load(None).expect("default config must parse");
        assert!(!config.columns.is_empty());
        assert_eq!(config.hours_per_day, Duration::from_minutes(480));
        assert_eq!(config.days_per_week, 5);
    }

    #[test]
    fn override_replaces_only_the_named_field() {
        let user = "schedule.days_per_week = 6";
        let config = Config::load(Some(&v(user))).expect("override must parse");
        assert_eq!(config.days_per_week, 6);
        assert_eq!(config.hours_per_day, Duration::from_minutes(480));
    }

    #[test]
    fn unknown_column_is_an_error() {
        let user = r#"columns.order = ["bogus"]"#;
        assert!(Config::load(Some(&v(user))).is_err());
    }

    #[test]
    fn default_styles_resolve() {
        let config = Config::load(None).expect("default config must parse");
        for status in [
            Status::Draft,
            Status::Todo,
            Status::Wip,
            Status::Paused,
            Status::Done,
        ] {
            assert!(config.glyph_colors.contains_key(&status));
        }
        assert_eq!(
            config.style_for(Semantic::Text(Status::Done)),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT)
        );
        assert_eq!(
            config.style_for(Semantic::Text(Status::Todo)),
            Style::default()
        );
        assert_eq!(
            config.style_for(Semantic::AggregateText(Status::Todo)),
            Style::default().add_modifier(Modifier::DIM)
        );
        assert_eq!(
            config.style_for(Semantic::Overdue),
            Style::default().fg(Color::Red)
        );
        assert_eq!(
            config.style_for(Semantic::Selected),
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn unknown_style_color_is_an_error() {
        let user = r#"styles.overdue_color = "bogus""#;
        assert!(Config::load(Some(&v(user))).is_err());
    }

    #[test]
    fn default_status_order_excludes_draft() {
        let config = Config::load(None).expect("default config must parse");
        assert_eq!(
            config.status_order,
            vec![Status::Todo, Status::Wip, Status::Paused, Status::Done]
        );
    }

    #[test]
    fn unknown_status_is_an_error() {
        let user = r#"status.enabled = ["bogus"]"#;
        assert!(Config::load(Some(&v(user))).is_err());
    }

    #[test]
    fn status_override_narrows_cycle_order() {
        let user = r#"status.enabled = ["todo", "wip", "done"]"#;
        let config = Config::load(Some(&v(user))).expect("override must parse");
        assert_eq!(
            config.status_order,
            vec![Status::Todo, Status::Wip, Status::Done]
        );
        // unspecified fields (glyphs) keep their embedded default
        assert!(config.status_glyphs.contains_key(&Status::Draft));
    }
}
