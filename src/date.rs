//! Calendar dates, by hand: there is no date crate here, and the few things
//! omh needs — today's date, a day count from one, and whether a string is
//! a date that exists — are short enough to be worth one module rather than
//! a dependency.

/// `\d{4}-\d{2}-\d{2}` is the rule as first written, and it is the weak
/// version: it accepts `2026-13-45`. A note's date is the only thing an
/// unverified claim can be judged by, so it has to be a date that exists.
pub(crate) fn is_calendar_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let (year, month, day) = (
        s[0..4].parse::<u32>().unwrap_or(0),
        s[5..7].parse::<u32>().unwrap_or(0),
        s[8..10].parse::<u32>().unwrap_or(0),
    );
    (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// `YYYY-MM-DD` for today, UTC.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil(secs / 86_400)
}

/// Days since the epoch to a civil date. Hand-rolled because there is no date
/// crate here, and pure so the calendar can be tested without a clock —
/// `recorded` is the field the base-set post-mortem was written about.
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since 1970-01-01, or `i64::MAX` if this machine's clock is before the
/// epoch — cannot-tell refuses nothing, the rule the rest of this codebase runs
/// on. A broken clock must not turn every measurement into a curation failure.
#[cfg(test)]
pub(crate) fn today_in_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(i64::MAX, |d| (d.as_secs() / 86_400) as i64)
}

/// Days since 1970-01-01 for a civil date — Howard Hinnant's `days_from_civil`,
/// exact across the proleptic Gregorian calendar and short enough not to be
/// worth a dependency for one comparison.
#[cfg(test)]
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the clock ───────────────────────────────────────────────────────────

    /// `days / 365` misdates every note by a growing amount, and it is exactly
    /// the class `contributing.md` counted four shipped bugs in: pure,
    /// cheaply-testable code that nobody tested.
    #[test]
    fn todays_date_is_computed_from_the_calendar_not_from_averages() {
        for (days, want) in [
            (0, "1970-01-01"),
            (19_722, "2023-12-31"),
            (19_723, "2024-01-01"),
            (19_782, "2024-02-29"), // a leap day
            (11_016, "2000-02-29"), // a century that IS a leap year
            (365, "1971-01-01"),
        ] {
            assert_eq!(civil(days), want, "day {days}");
        }
    }

    /// The pure table above cannot see an epoch offset or seconds read as
    /// days. Only the system can.
    #[test]
    #[cfg(unix)]
    fn todays_date_agrees_with_the_system_clock() {
        let out = std::process::Command::new("date")
            .args(["-u", "+%F"])
            .output()
            .unwrap();
        let want = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(today(), want);
    }
}
