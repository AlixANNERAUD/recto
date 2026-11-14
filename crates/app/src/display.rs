//! Times and sizes as the interface writes them. The same code runs on the server and in the
//! browser, so an island's first render matches the HTML it takes over.

use crate::{t, tf, tp};

/// Seconds since the Unix epoch, from the server's clock or the browser's.
pub fn unix_now() -> i64 {
    #[cfg(feature = "hydrate")]
    {
        // Milliseconds as a float; whole seconds fit an i64 for any date a job can have.
        #[allow(clippy::cast_possible_truncation)]
        let seconds = (js_sys::Date::now() / 1000.0) as i64;
        seconds
    }
    #[cfg(not(feature = "hydrate"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
            })
    }
}

/// `(year, month, day, hour, minute)` in UTC of a Unix time (Howard Hinnant's `civil_from_days`).
fn civil(unix: i64) -> (i64, i64, i64, i64, i64) {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day, seconds / 3600, seconds % 3600 / 60)
}

/// The machine-readable form for `<time datetime>`.
pub fn iso_time(unix: i64) -> String {
    let (year, month, day, hour, minute) = civil(unix);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}Z")
}

/// How long ago something happened, for times within a day (`5 minutes ago`); older ones are
/// a date and time in UTC, which the server and every reader agree on.
pub fn relative_time(unix: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(unix);
    match elapsed {
        ..60 => t!("time.just_now").to_string(),
        60..3600 => tp!("time.minutes_ago", elapsed / 60),
        3600..86_400 => tp!("time.hours_ago", elapsed / 3600),
        _ => {
            let (year, month, day, hour, minute) = civil(unix);
            tf!(
                "time.utc",
                time = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
            )
        }
    }
}

/// A size in bytes as `50 MB` (binary megabytes, as `RECTO_MAX_UPLOAD_SIZE` counts them).
pub fn file_size(bytes: usize) -> String {
    const MEGABYTE: usize = 1024 * 1024;
    const KILOBYTE: usize = 1024;
    if bytes >= MEGABYTE {
        let tenths = bytes * 10 / MEGABYTE;
        let value = if tenths.is_multiple_of(10) {
            (tenths / 10).to_string()
        } else {
            format!("{}.{}", tenths / 10, tenths % 10)
        };
        tf!("size.megabytes", value = value)
    } else {
        tf!("size.kilobytes", value = bytes.div_ceil(KILOBYTE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_utc_dates() {
        assert_eq!(iso_time(0), "1970-01-01T00:00Z");
        assert_eq!(iso_time(1_700_000_000), "2023-11-14T22:13Z");
        assert_eq!(iso_time(951_782_400), "2000-02-29T00:00Z");
    }

    #[test]
    fn writes_relative_times() {
        let now = 1_700_000_000;
        assert_eq!(relative_time(now - 10, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 7200, now), "2 hours ago");
        assert_eq!(relative_time(now - 86_400, now), "2023-11-13 22:13 UTC");
        // A clock running behind the printer's is still "just now".
        assert_eq!(relative_time(now + 30, now), "just now");
    }

    #[test]
    fn writes_sizes() {
        assert_eq!(file_size(50 * 1024 * 1024), "50 MB");
        assert_eq!(file_size(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(file_size(4096), "4 KB");
    }
}
