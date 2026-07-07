//! Work-day projection for the tree-table `eta` column: given remaining
//! effort and a work calendar (hours/day, days/week), project the calendar
//! date the remaining work finishes.

use todoapp_core::{Date, Duration};

/// Overrun-aware remaining effort: re-bases on how many multiples of
/// `estimate` the `elapsed` time has already consumed, so a task that ran
/// past its estimate still projects a positive remaining duration instead of
/// going negative. `remaining = n * estimate - elapsed`, `n = max(1, ceil(elapsed / estimate))`.
pub fn remaining_effort(estimate: Duration, elapsed: Duration) -> Duration {
    if estimate == Duration::ZERO {
        return Duration::ZERO;
    }
    let n = elapsed.as_minutes().div_ceil(estimate.as_minutes()).max(1);
    Duration::from_minutes(n * estimate.as_minutes() - elapsed.as_minutes())
}

/// The date `remaining` work finishes, starting from `today`, at
/// `hours_per_day` capacity and `days_per_week` workdays.
///
/// ponytail: Mon-anchored N-day work week (`Mon..Mon+days_per_week` are
/// workdays) — no per-weekday selection. Add an explicit weekday set if
/// someone needs e.g. Tue-Sat.
pub fn project_finish_date(
    today: Date,
    remaining: Duration,
    hours_per_day: Duration,
    days_per_week: u8,
) -> Date {
    if remaining == Duration::ZERO || hours_per_day == Duration::ZERO {
        return today;
    }
    let per_day = hours_per_day.as_minutes();
    let work_days_needed = remaining.as_minutes().div_ceil(per_day);

    let mut date = today.0;
    let mut remaining_days = work_days_needed;
    while remaining_days > 0 {
        date = date.tomorrow().unwrap_or(date);
        let is_workday =
            date.weekday().to_monday_zero_offset() < i8::try_from(days_per_week).unwrap_or(i8::MAX);
        if is_workday {
            remaining_days -= 1;
        }
    }
    Date(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_effort_not_started_is_full_estimate() {
        assert_eq!(
            remaining_effort(Duration::from_minutes(60), Duration::ZERO),
            Duration::from_minutes(60)
        );
    }

    #[test]
    fn remaining_effort_on_track_subtracts_elapsed() {
        assert_eq!(
            remaining_effort(Duration::from_minutes(60), Duration::from_minutes(20)),
            Duration::from_minutes(40)
        );
    }

    #[test]
    fn remaining_effort_exact_is_zero() {
        assert_eq!(
            remaining_effort(Duration::from_minutes(60), Duration::from_minutes(60)),
            Duration::ZERO
        );
    }

    #[test]
    fn remaining_effort_overrun_rebases_on_next_multiple() {
        // elapsed = 90m over a 60m estimate: ceil(90/60) = 2 → n=2, remaining = 120-90 = 30m.
        assert_eq!(
            remaining_effort(Duration::from_minutes(60), Duration::from_minutes(90)),
            Duration::from_minutes(30)
        );
    }

    #[test]
    fn remaining_effort_zero_estimate_is_zero() {
        assert_eq!(
            remaining_effort(Duration::ZERO, Duration::from_minutes(30)),
            Duration::ZERO
        );
    }

    #[test]
    fn zero_remaining_finishes_today() {
        let today = Date::parse("2026-06-22").unwrap(); // Monday
        assert_eq!(
            project_finish_date(today, Duration::ZERO, Duration::from_minutes(480), 5),
            today
        );
    }

    #[test]
    fn multi_day_estimate_skips_the_weekend() {
        // Monday 2026-06-22, 5-day work week, 8h/day capacity, 3 work days of
        // effort (24h) starting tomorrow → Tue+Wed+Thu, finishes Thursday.
        let today = Date::parse("2026-06-22").unwrap();
        let finish = project_finish_date(
            today,
            Duration::from_minutes(24 * 60),
            Duration::from_minutes(480),
            5,
        );
        assert_eq!(finish, Date::parse("2026-06-25").unwrap());

        // 5 work days of effort starting Thursday must cross the weekend.
        let thursday = Date::parse("2026-06-25").unwrap();
        let finish = project_finish_date(
            thursday,
            Duration::from_minutes(5 * 480),
            Duration::from_minutes(480),
            5,
        );
        assert_eq!(finish, Date::parse("2026-07-02").unwrap());
    }
}
