//! Typed date/time/duration domain values (spec §3): `jiff` for instants and
//! calendar dates, `std::time::Duration` for durations — never raw `String`/
//! `u32`. Each wrapper's `Serialize`/`Deserialize` matches the on-wire shape
//! the fields had before typing (bare integer / ISO date string), so adapters
//! that bridge components through `serde_json` (`todoapp-store-turso`) need no
//! schema changes.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An instant (task `created_at`/`updated_at`). Serializes as epoch
/// milliseconds (a bare integer), matching the pre-jiff `Timestamp(i64)` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub jiff::Timestamp);

impl Timestamp {
    pub fn as_millisecond(&self) -> i64 {
        self.0.as_millisecond()
    }
    pub fn from_millisecond(ms: i64) -> Self {
        Self(jiff::Timestamp::from_millisecond(ms).unwrap_or(jiff::Timestamp::UNIX_EPOCH))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.as_millisecond())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_millisecond(i64::deserialize(d)?))
    }
}

/// A calendar date (`Schedule`/due dates). Serializes as an ISO-8601
/// `YYYY-MM-DD` string (jiff's own serde impl) — the same shape the field had
/// as a raw `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Date(pub jiff::civil::Date);

impl Date {
    pub fn parse(s: &str) -> Result<Self, jiff::Error> {
        s.parse::<jiff::civil::Date>().map(Date)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A time-of-day (minute precision), for a due date's optional rendez-vous
/// time. Naive/local, like the rest of the codebase — no zone is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(pub jiff::civil::Time);

impl Time {
    /// Accepts `"HH:MM"` (24h).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (h, m) = s
            .split_once(':')
            .ok_or_else(|| format!("expected \"HH:MM\", got {s:?}"))?;
        let h: i8 = h.parse().map_err(|_| format!("bad hour in {s:?}"))?;
        let m: i8 = m.parse().map_err(|_| format!("bad minute in {s:?}"))?;
        jiff::civil::Time::new(h, m, 0, 0)
            .map(Time)
            .map_err(|e| format!("bad time {s:?}: {e}"))
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0.hour(), self.0.minute())
    }
}

// jscpd:ignore-start
// ponytail: same 7-line Serialize/Deserialize-via-Display/parse shape as `Due`
// below; only 2 occurrences and each is tied to its own type's `Display`/
// `parse`, so a macro would cost more readability than the duplication does.
// Promote to a macro if a third string-roundtrip type shows up.
impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Time::parse(&s).map_err(serde::de::Error::custom)
    }
}
// jscpd:ignore-end

/// A due date with an optional time-of-day (spec: a "rendez-vous" due can
/// carry a time; overdue/eta rollups (`Aggregate::earliest_due`,
/// `project_finish_date`) stay day-granularity and only ever read `.date` —
/// `.time` is display-only, never compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Due {
    pub date: Date,
    pub time: Option<Time>,
}

impl Due {
    /// Accepts `"YYYY-MM-DD"` or `"YYYY-MM-DD HH:MM"` (space or `T` separated).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (date_part, time_part) = match s.split_once([' ', 'T']) {
            Some((d, t)) => (d, Some(t)),
            None => (s, None),
        };
        let date = Date::parse(date_part).map_err(|e| format!("bad date {date_part:?}: {e}"))?;
        let time = time_part.map(Time::parse).transpose()?;
        Ok(Due { date, time })
    }
}

impl From<Date> for Due {
    fn from(date: Date) -> Self {
        Due { date, time: None }
    }
}

impl fmt::Display for Due {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.time {
            Some(t) => write!(f, "{} {t}", self.date),
            None => write!(f, "{}", self.date),
        }
    }
}

// jscpd:ignore-start
// ponytail: same shape as `Time` above — see the note there.
impl Serialize for Due {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Due {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Due::parse(&s).map_err(serde::de::Error::custom)
    }
}
// jscpd:ignore-end

/// An effort/elapsed duration (`Estimate`, `TimeSpent`), minute precision.
/// Serializes as an integer number of minutes — the same shape the field had
/// as a raw `u32`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(pub std::time::Duration);

impl Duration {
    pub const ZERO: Self = Self(std::time::Duration::ZERO);

    pub fn from_minutes(m: u32) -> Self {
        Self(std::time::Duration::from_secs(u64::from(m) * 60))
    }

    pub fn as_minutes(&self) -> u32 {
        u32::try_from(self.0.as_secs() / 60).unwrap_or(u32::MAX)
    }
}

impl std::ops::Add for Duration {
    type Output = Duration;
    fn add(self, rhs: Self) -> Duration {
        Duration(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for Duration {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl std::ops::Sub for Duration {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        Duration(self.0.saturating_sub(rhs.0))
    }
}

impl std::iter::Sum for Duration {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Duration::ZERO, std::ops::Add::add)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.as_minutes();
        match (total / 60, total % 60) {
            (0, 0) => write!(f, "0m"),
            (0, m) => write!(f, "{m}m"),
            (h, 0) => write!(f, "{h}h"),
            (h, m) => write!(f, "{h}h{m}m"),
        }
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.as_minutes())
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_minutes(u32::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_display_is_human_readable() {
        assert_eq!(Duration::ZERO.to_string(), "0m");
        assert_eq!(Duration::from_minutes(45).to_string(), "45m");
        assert_eq!(Duration::from_minutes(60).to_string(), "1h");
        assert_eq!(Duration::from_minutes(90).to_string(), "1h30m");
    }

    #[test]
    fn duration_json_roundtrips_as_integer_minutes() {
        let d = Duration::from_minutes(30);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "30");
        assert_eq!(serde_json::from_str::<Duration>(&json).unwrap(), d);
    }

    #[test]
    fn date_json_roundtrips_as_iso_string() {
        let d = Date::parse("2026-07-01").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"2026-07-01\"");
        assert_eq!(serde_json::from_str::<Date>(&json).unwrap(), d);
    }

    #[test]
    fn due_parses_date_only_and_date_time() {
        let date_only = Due::parse("2026-07-01").unwrap();
        assert_eq!(date_only.time, None);
        assert_eq!(date_only.to_string(), "2026-07-01");

        let with_time = Due::parse("2026-07-01 14:30").unwrap();
        assert_eq!(with_time.date, date_only.date);
        assert_eq!(with_time.to_string(), "2026-07-01 14:30");

        // legacy plain-date rows (no time) still parse under the richer type.
        assert_eq!(Due::parse("2026-07-01T14:30").unwrap(), with_time);
    }

    #[test]
    fn due_json_roundtrips_and_orders_date_before_time() {
        let with_time = Due::parse("2026-07-01 09:00").unwrap();
        let json = serde_json::to_string(&with_time).unwrap();
        assert_eq!(json, "\"2026-07-01 09:00\"");
        assert_eq!(serde_json::from_str::<Due>(&json).unwrap(), with_time);

        let date_only: Due = Date::parse("2026-07-01").unwrap().into();
        assert!(
            date_only < with_time,
            "a bare due date sorts before a same-day rendez-vous time"
        );
    }
}
