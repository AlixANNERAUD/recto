//! The window of time a rule counts usage in.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use super::Period;

/// The window of `period` that contains `now`, as `[start, end)`; `None` is unbounded.
///
/// Calendar periods (day, week from Monday, month, quarter from January, April, July and
/// October, year) start and end at local midnight in `tz`, so a day can last 23 or 25 hours.
/// `Rolling(n)` is the last `n` × 24 hours, and `Total` never resets.
pub fn window(
    period: Period,
    now: DateTime<Utc>,
    tz: Tz,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let today = now.with_timezone(&tz).date_naive();
    let (start, end) = match period {
        Period::Total => return (None, None),
        Period::Rolling(days) => return (Some(now - Duration::days(i64::from(days))), None),
        Period::Day => (today, today + Duration::days(1)),
        Period::Week => {
            let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
            (monday, monday + Duration::days(7))
        }
        Period::Month => {
            let first = first_of(today.year(), today.month());
            (first, months_later(first, 1))
        }
        Period::Quarter => {
            let first = first_of(today.year(), (today.month() - 1) / 3 * 3 + 1);
            (first, months_later(first, 3))
        }
        Period::Year => {
            let first = first_of(today.year(), 1);
            (first, months_later(first, 12))
        }
    };
    (Some(midnight(tz, start)), Some(midnight(tz, end)))
}

fn first_of(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).expect("the first of a month exists")
}

fn months_later(first: NaiveDate, months: u32) -> NaiveDate {
    first
        .checked_add_months(chrono::Months::new(months))
        .expect("a date within chrono's range")
}

/// The instant a local day starts. Where a change of clocks skips midnight, the day starts at
/// the first local time that exists; where midnight happens twice, at the first.
fn midnight(tz: Tz, date: NaiveDate) -> DateTime<Utc> {
    let mut time = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    // Clock changes skip at most a few hours.
    for _ in 0..(24 * 4) {
        match tz.from_local_datetime(&time) {
            LocalResult::Single(instant) | LocalResult::Ambiguous(instant, _) => {
                return instant.with_timezone(&Utc);
            }
            LocalResult::None => time += Duration::minutes(15),
        }
    }
    // Unreachable with real zones: read the date as UTC rather than fail.
    date.and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Paris;

    fn utc(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn calendar_windows_start_at_local_midnight() {
        // The day clocks go forward in Paris: it starts at 23:00 UTC the day before, in winter
        // time, and lasts 23 hours.
        let now = utc("2026-03-29T12:00:00Z");
        assert_eq!(
            window(Period::Day, now, Paris),
            (
                Some(utc("2026-03-28T23:00:00Z")),
                Some(utc("2026-03-29T22:00:00Z"))
            )
        );
        let now = utc("2026-11-17T10:00:00Z");
        assert_eq!(
            window(Period::Month, now, Paris),
            (
                Some(utc("2026-10-31T23:00:00Z")),
                Some(utc("2026-11-30T23:00:00Z"))
            )
        );
        // Wednesday 18 November: the week started on Monday the 16th.
        let wednesday = utc("2026-11-18T09:00:00Z");
        assert_eq!(
            window(Period::Week, wednesday, Paris),
            (
                Some(utc("2026-11-15T23:00:00Z")),
                Some(utc("2026-11-22T23:00:00Z"))
            )
        );
        // May is in the quarter of April, which starts in summer time.
        let may = utc("2026-05-20T12:00:00Z");
        assert_eq!(
            window(Period::Quarter, may, Paris),
            (
                Some(utc("2026-03-31T22:00:00Z")),
                Some(utc("2026-06-30T22:00:00Z"))
            )
        );
        assert_eq!(
            window(Period::Year, may, Paris),
            (
                Some(utc("2025-12-31T23:00:00Z")),
                Some(utc("2026-12-31T23:00:00Z"))
            )
        );
        // Late on 31 December in UTC is already the next year in Paris.
        let new_year = utc("2026-12-31T23:30:00Z");
        assert_eq!(
            window(Period::Year, new_year, Paris).0,
            Some(utc("2026-12-31T23:00:00Z"))
        );
        assert_eq!(
            window(Period::Day, now, chrono_tz::UTC),
            (
                Some(utc("2026-11-17T00:00:00Z")),
                Some(utc("2026-11-18T00:00:00Z"))
            )
        );
    }

    #[test]
    fn a_skipped_midnight_starts_the_day_at_the_first_local_time() {
        // Santiago moved its clocks forward at midnight on 7 September 2025: 00:00 did not
        // exist, the day began at 01:00 local, 04:00 UTC.
        let now = utc("2025-09-07T15:00:00Z");
        assert_eq!(
            window(Period::Day, now, chrono_tz::America::Santiago).0,
            Some(utc("2025-09-07T04:00:00Z"))
        );
    }

    #[test]
    fn rolling_and_total_windows() {
        let now = utc("2026-11-17T10:00:00Z");
        assert_eq!(
            window(Period::Rolling(7), now, Paris),
            (Some(utc("2026-11-10T10:00:00Z")), None)
        );
        assert_eq!(window(Period::Total, now, Paris), (None, None));
    }
}
