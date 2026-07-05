//! General TUI config: tree-table column order/visibility + the work
//! calendar used to project the `eta` column. Same embedded-default +
//! user-override TOML pattern as [`crate::keymap`]: an optional user file at
//! `$TDA_CONFIG` (or `~/.config/tda/config.toml`) overrides individual
//! fields; unmentioned fields keep their embedded defaults.

use std::collections::BTreeMap;

use anyhow::{Context as _, bail};
use serde::Deserialize;
use todoapp_core::{Duration, Status};

const DEFAULT_CONFIG_TOML: &str = include_str!("../config.default.toml");

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
struct RawConfig {
    #[serde(default)]
    columns: RawColumns,
    #[serde(default)]
    schedule: RawSchedule,
    #[serde(default)]
    status: RawStatus,
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
}

impl Config {
    /// Load defaults, then apply `user_toml` overrides (if given) on top —
    /// each present field replaces its default; unmentioned fields keep their
    /// embedded default.
    pub fn load(user_toml: Option<&str>) -> anyhow::Result<Self> {
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

        if let Some(user_toml) = user_toml {
            let overrides: RawConfig = toml::from_str(user_toml).context("parse user config")?;
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = Config::load(Some(user)).expect("override must parse");
        assert_eq!(config.days_per_week, 6);
        assert_eq!(config.hours_per_day, Duration::from_minutes(480));
    }

    #[test]
    fn unknown_column_is_an_error() {
        let user = r#"columns.order = ["bogus"]"#;
        assert!(Config::load(Some(user)).is_err());
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
        assert!(Config::load(Some(user)).is_err());
    }

    #[test]
    fn status_override_narrows_cycle_order() {
        let user = r#"status.enabled = ["todo", "wip", "done"]"#;
        let config = Config::load(Some(user)).expect("override must parse");
        assert_eq!(
            config.status_order,
            vec![Status::Todo, Status::Wip, Status::Done]
        );
        // unspecified fields (glyphs) keep their embedded default
        assert!(config.status_glyphs.contains_key(&Status::Draft));
    }
}
