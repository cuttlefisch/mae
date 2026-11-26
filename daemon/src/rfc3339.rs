//! Unix seconds as an RFC 3339 UTC timestamp (`2020-09-13T12:26:40Z`), for
//! the times `kb/query` reports.
//!
//! @ai-caution: [architecture-debt] The fourth civil-date conversion in the
//! repository (`mae-core` `kb_ops::civil_from_days`, `mae-core` `project.rs`,
//! `mae-kb` `migrate`). None is reachable from the daemon without a dependency
//! it does not otherwise need; consolidating them into one shared helper is the
//! fix, not a fifth copy.

/// `secs` as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Unix milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ` (ADR-113 E9's times).
pub fn utc_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let base = utc(secs);
    format!("{}.{:03}Z", &base[..base.len() - 1], ms.rem_euclid(1000))
}

/// The inverse of [`utc_millis`], strictly: exactly
/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, a real calendar date, as unix milliseconds.
/// `None` for anything else, so a time the API did not write is refused
/// rather than guessed at.
pub fn parse_utc_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'.'
        || b[23] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss, ms) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..23)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    // A day past the month's end (31 April) does not round-trip: refuse it.
    if civil_from_days(days) != (y, m as u32, d as u32) {
        return None;
    }
    Some(((days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000) + ms)
}

/// Howard Hinnant's `days_from_civil`: (y, m, d) to days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    /// Oracle: `date -u -d @N +%Y-%m-%dT%H:%M:%SZ`, including a leap day, the
    /// second before the epoch and the end of a century.
    #[test]
    fn matches_date_on_boundaries() {
        for (secs, want) in [
            (0, "1970-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_600_000_000, "2020-09-13T12:26:40Z"),
            (-1, "1969-12-31T23:59:59Z"),
            (4_102_444_799, "2099-12-31T23:59:59Z"),
        ] {
            assert_eq!(super::utc(secs), want);
        }
    }

    /// Milliseconds are zero-padded, and an instant before the epoch rounds
    /// down to its second, as `date -u -d @-0.001 +%T.%3N` does.
    #[test]
    fn millis_are_padded_and_floor_before_the_epoch() {
        for (ms, want) in [
            (0, "1970-01-01T00:00:00.000Z"),
            (1_600_000_000_007, "2020-09-13T12:26:40.007Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
        ] {
            assert_eq!(super::utc_millis(ms), want);
        }
    }
}

#[cfg(test)]
mod parse_tests {
    use super::{parse_utc_millis, utc_millis};

    /// Every millisecond value written by `utc_millis` parses back to itself,
    /// over a seeded spread from 1970 to 2100, leap days included.
    #[test]
    fn what_utc_millis_writes_parses_back() {
        let mut x: u64 = 0x2026_1125;
        for _ in 0..5000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let ms = (x % 4_102_444_800_000) as i64;
            assert_eq!(parse_utc_millis(&utc_millis(ms)), Some(ms), "{ms}");
        }
        assert_eq!(
            parse_utc_millis("2024-02-29T00:00:00.000Z"),
            Some(1_709_164_800_000)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        for s in [
            "2026-01-22T09:00:00Z",
            "2026-01-22T09:00:00.000+01:00",
            "2026-04-31T00:00:00.000Z",
            "2025-02-29T00:00:00.000Z",
            "2026-13-01T00:00:00.000Z",
            "2026-01-22T24:00:00.000Z",
            "2026-01-22 09:00:00.000Z",
            "+026-01-22T09:00:00.000Z",
            "",
        ] {
            assert_eq!(parse_utc_millis(s), None, "{s}");
        }
    }
}
