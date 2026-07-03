//! Typed date/time/duration domain values (spec §3): `jiff` for instants and
//! calendar dates, `std::time::Duration` for durations — never raw `String`/
//! `u32`. Each wrapper's `Serialize`/`Deserialize` matches the on-wire shape
//! the fields had before typing (bare integer / ISO date string), so adapters
//! that bridge components through `serde_json` (`tda-store-turso`) need no
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
}
