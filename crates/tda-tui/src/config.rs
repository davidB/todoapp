//! General TUI config: tree-table column order/visibility + the work
//! calendar used to project the `eta` column. Same embedded-default +
//! user-override TOML pattern as [`crate::keymap`]: an optional user file at
//! `$TDA_CONFIG` (or `~/.config/tda/config.toml`) overrides individual
//! fields; unmentioned fields keep their embedded defaults.

use anyhow::{Context as _, bail};
use serde::Deserialize;
use tda_core::Duration;

const DEFAULT_CONFIG_TOML: &str = include_str!("../config.default.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Status,
    Due,
    Eta,
    Assignee,
    Estimate,
    Elapsed,
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
struct RawConfig {
    #[serde(default)]
    columns: RawColumns,
    #[serde(default)]
    schedule: RawSchedule,
}

pub struct Config {
    pub columns: Vec<ColumnKind>,
    pub hours_per_day: Duration,
    pub days_per_week: u8,
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

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let minutes = (hours_per_day * 60.0).round() as u32;
        Ok(Self {
            columns,
            hours_per_day: Duration::from_minutes(minutes),
            days_per_week,
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
}
