//! Parse/format an estimate as human "work time" — `1d 2h 30m`, where a day
//! is `hours_per_day` and a week is `days_per_week` days (config-driven, so
//! `1d` means one configured workday, not 24h).

use std::fmt::Write as _;

use tda_core::Duration;

/// `90m`/`90` (bare minutes, back-compat) or `1w 2d 3h 30m` in any order/spacing.
pub fn parse(s: &str, hours_per_day: Duration, days_per_week: u8) -> Result<Duration, String> {
    let day_minutes = hours_per_day.as_minutes().max(1);
    let week_minutes = day_minutes * u32::from(days_per_week.max(1));
    let mut total: u32 = 0;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        let n: u32 = num
            .parse()
            .map_err(|_| format!("expected a number before {c:?}"))?;
        num.clear();
        total += match c.to_ascii_lowercase() {
            'w' => n * week_minutes,
            'd' => n * day_minutes,
            'h' => n * 60,
            'm' => n,
            other => return Err(format!("unknown time unit {other:?} (use w/d/h/m)")),
        };
    }
    if !num.is_empty() {
        // trailing bare number with no unit: back-compat with plain minutes.
        total += num
            .parse::<u32>()
            .map_err(|_| format!("bad number {num:?}"))?;
    }
    Ok(Duration::from_minutes(total))
}

/// Formats using only the units that fit (largest first), e.g. `1d3h`.
pub fn format(d: Duration, hours_per_day: Duration, days_per_week: u8) -> String {
    let day_minutes = hours_per_day.as_minutes().max(1);
    let week_minutes = day_minutes * u32::from(days_per_week.max(1));
    let mut total = d.as_minutes();
    let weeks = total / week_minutes;
    total %= week_minutes;
    let days = total / day_minutes;
    total %= day_minutes;
    let hours = total / 60;
    let minutes = total % 60;

    let mut out = String::new();
    if weeks > 0 {
        let _ = write!(out, "{weeks}w");
    }
    if days > 0 {
        let _ = write!(out, "{days}d");
    }
    if hours > 0 {
        let _ = write!(out, "{hours}h");
    }
    if minutes > 0 || out.is_empty() {
        let _ = write!(out, "{minutes}m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> Duration {
        Duration::from_minutes(8 * 60)
    }

    #[test]
    fn parses_and_formats_work_units() {
        // 1 day + 1 hour + 30 min, with an 8h/day, 5-day week.
        let d = parse("1d 1h 30m", day(), 5).unwrap();
        assert_eq!(d, Duration::from_minutes(8 * 60 + 90));
        assert_eq!(format(d, day(), 5), "1d1h30m");
    }

    #[test]
    fn a_week_is_days_per_week_days() {
        let d = parse("1w", day(), 5).unwrap();
        assert_eq!(d, Duration::from_minutes(5 * 8 * 60));
    }

    #[test]
    fn bare_number_is_back_compat_minutes() {
        assert_eq!(parse("90", day(), 5).unwrap(), Duration::from_minutes(90));
    }

    #[test]
    fn unknown_unit_is_an_error() {
        assert!(parse("3x", day(), 5).is_err());
    }
}
