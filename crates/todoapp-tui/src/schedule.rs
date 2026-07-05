//! Work-day projection for the tree-table `eta` column: given remaining
//! effort and a work calendar (hours/day, days/week), project the calendar
//! date the remaining work finishes.

use todoapp_core::{Date, Duration};

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
