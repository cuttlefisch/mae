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
}
