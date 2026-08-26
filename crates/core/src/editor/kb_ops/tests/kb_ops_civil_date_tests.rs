//! `chrono_now()`'s calendar arithmetic.
//!
//! The previous implementation used 365-day years and 30-day months with no
//! leap handling. These tests are written against the failures it actually
//! produced, not against a generic "dates should be right" idea:
//!
//! - it was 17 days ahead by 2026, and the drift GREW rather than staying a
//!   fixed offset, so a single spot-check would have looked fine for a while;
//! - `remainder_days` reached 364, so `months` reached 13 and it emitted
//!   `2026-13-01` -- not a wrong date, an impossible one.

use super::*;

/// Known epoch-day/date pairs, chosen for the cases the old code got wrong
/// rather than for round numbers.
#[test]
fn civil_from_days_matches_known_dates() {
    // (days since 1970-01-01, y, m, d)
    let cases: &[(i64, i64, u32, u32)] = &[
        (0, 1970, 1, 1),       // epoch
        (-1, 1969, 12, 31),    // before the epoch
        (59, 1970, 3, 1),      // no leap day in 1970
        (19_723, 2024, 1, 1),  // a leap year's start
        (19_782, 2024, 2, 29), // THE leap day -- the old code had no concept of it
        (19_783, 2024, 3, 1),  // and the day after
        (20_088, 2024, 12, 31),
        (20_089, 2025, 1, 1),  // year boundary
        (11_016, 2000, 2, 29), // 2000 IS a leap year (divisible by 400)
        (-25_567, 1900, 1, 1), // 1900 is NOT (divisible by 100, not 400)
        (20_691, 2026, 8, 26),
    ];
    for &(days, y, m, d) in cases {
        assert_eq!(
            civil_from_days(days),
            (y, m, d),
            "day {days} must be {y:04}-{m:02}-{d:02}"
        );
    }
}

/// The invalid-date bug, stated as the property it violated: every day of a
/// decade must produce a real calendar month. The old code failed this on
/// roughly five days each December.
#[test]
fn civil_from_days_never_emits_an_impossible_month_or_day() {
    // 1970-01-01 through ~2040.
    for days in 0..25_600i64 {
        let (y, m, d) = civil_from_days(days);
        assert!(
            (1..=12).contains(&m),
            "day {days} produced month {m} ({y:04}-{m:02}-{d:02})"
        );
        assert!(
            (1..=31).contains(&d),
            "day {days} produced day {d} ({y:04}-{m:02}-{d:02})"
        );
    }
}

/// Round-trip against an independent implementation of the inverse, so the
/// oracle is not the same arithmetic restated. Consecutive days must also be
/// strictly increasing dates -- the old code was not even monotonic across a
/// year boundary, since `remainder_days` wrapped.
#[test]
fn civil_from_days_round_trips_and_is_monotonic() {
    fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
        let y = y - i64::from(m <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = i64::from(m) + if m > 2 { -3 } else { 9 };
        let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
    let mut prev: Option<(i64, u32, u32)> = None;
    for days in -1_000..25_600i64 {
        let (y, m, d) = civil_from_days(days);
        assert_eq!(
            days_from_civil(y, m, d),
            days,
            "round-trip failed at {days}"
        );
        if let Some(p) = prev {
            assert!((y, m, d) > p, "dates went backwards at day {days}");
        }
        prev = Some((y, m, d));
    }
}
