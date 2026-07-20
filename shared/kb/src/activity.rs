//! Activity tracking — or-east parity.
//!
//! Computes activity-decay scores from node property timestamps.
//! Score formula: `Σ(weight * 1/(1 + decay * age_days))` for each
//! tracked timestamp (last-accessed, last-modified, last-linked).

use std::collections::HashMap;

/// Weights for the activity score components.
pub struct ActivityWeights {
    pub accessed: f64,
    pub modified: f64,
    pub linked: f64,
    pub decay: f64,
}

impl Default for ActivityWeights {
    fn default() -> Self {
        ActivityWeights {
            accessed: 1.0,
            modified: 2.0,
            linked: 0.5,
            decay: 0.01,
        }
    }
}

/// Parse a `YYYY-MM-DD` date string. No chrono dependency.
pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Format a date as `YYYY-MM-DD`.
pub fn format_date(y: i32, m: u32, d: u32) -> String {
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Convert (y, m, d) to a day number for difference calculation.
/// Uses a simplified Julian Day algorithm.
fn to_day_number(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    // Algorithm from https://en.wikipedia.org/wiki/Julian_day#Converting_Gregorian_calendar_date_to_Julian_Day_Number
    let a = (14 - m) / 12;
    let y2 = y + 4800 - a;
    let m2 = m + 12 * a - 3;
    d + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045
}

/// Days between two dates. Returns absolute difference.
pub fn days_between(a: (i32, u32, u32), b: (i32, u32, u32)) -> i64 {
    (to_day_number(b.0, b.1, b.2) - to_day_number(a.0, a.1, a.2)).abs()
}

/// Step one day forward from (y, m, d).
pub fn next_day(y: i32, m: u32, d: u32) -> (i32, u32, u32) {
    let days_in_month = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 31,
    };
    if d < days_in_month {
        (y, m, d + 1)
    } else if m < 12 {
        (y, m + 1, 1)
    } else {
        (y + 1, 1, 1)
    }
}

/// Step one day backward from (y, m, d).
pub fn prev_day(y: i32, m: u32, d: u32) -> (i32, u32, u32) {
    if d > 1 {
        (y, m, d - 1)
    } else if m > 1 {
        let prev_m = m - 1;
        let prev_d = match prev_m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                    29
                } else {
                    28
                }
            }
            _ => 31,
        };
        (y, prev_m, prev_d)
    } else {
        (y - 1, 12, 31)
    }
}

/// Compute activity score from node properties.
/// Higher scores = more recently/frequently used nodes.
pub fn activity_score(
    props: &HashMap<String, String>,
    weights: &ActivityWeights,
    today: (i32, u32, u32),
) -> f64 {
    let mut score = 0.0;

    if let Some(date_str) = props.get("last-accessed") {
        if let Some(date) = parse_date(date_str) {
            let age = days_between(date, today) as f64;
            score += weights.accessed / (1.0 + weights.decay * age);
        }
    }

    if let Some(date_str) = props.get("last-modified") {
        if let Some(date) = parse_date(date_str) {
            let age = days_between(date, today) as f64;
            score += weights.modified / (1.0 + weights.decay * age);
        }
    }

    if let Some(date_str) = props.get("last-linked") {
        if let Some(date) = parse_date(date_str) {
            let age = days_between(date, today) as f64;
            score += weights.linked / (1.0 + weights.decay * age);
        }
    }

    score
}

/// Compute a simple body hash (FNV-1a-like) for change detection. Strips the
/// node's OWN `:PROPERTIES:...:END:` drawer (the volatile part —
/// `:hash:`/`:last-modified:` live there, and re-hashing them would never
/// converge) and hashes everything else, so the result reflects the node's
/// actual descriptive content regardless of whether that text comes before
/// the drawer (a list item: `"1. Text\n:PROPERTIES:...:END:"`) or after it
/// (a heading: `"* Heading\n:PROPERTIES:...:END:\nBody text"`) — hashing
/// only content AFTER the first `:END:` (the original implementation)
/// silently produced a constant/empty hash for every list item, since nothing
/// meaningful follows a list item's own drawer.
///
/// `content` MUST already be scoped to a single node's own body (e.g.
/// `org::parse_org_multi(file_content)`'s per-node `Node.body`, or
/// `parse_org_multi_with_types`'s), never a whole multi-node file's raw
/// content. A file can hold several nodes sharing one `source_file`
/// (file-level, per-heading, per-list-item — see #332); hashing the whole
/// file regardless of which node's properties are being checked silently
/// attributes one node's content change to another (or vice versa — editing
/// node B rewrites node A's `:hash:`). Only the FIRST drawer found is
/// stripped — a heading's own body legitimately includes its nested
/// children's raw text (drawers included), so a heading's hash can still
/// shift when a nested child's stamp changes; siblings (the reported bug
/// shape) are fully independent since their scoped bodies never overlap.
pub fn body_hash(content: &str) -> String {
    let stripped = match (content.find(":PROPERTIES:"), content.find(":END:")) {
        (Some(start), Some(end_marker)) if end_marker > start => {
            let end = end_marker + ":END:".len();
            // Skip past :END: and any trailing whitespace/newline before
            // resuming the hashed content.
            let rest = &content[end..];
            let resume = end + rest.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            format!("{}{}", &content[..start], &content[resume..])
        }
        _ => content.to_string(),
    };
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in stripped.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_valid() {
        assert_eq!(parse_date("2026-05-15"), Some((2026, 5, 15)));
        assert_eq!(parse_date("2000-01-01"), Some((2000, 1, 1)));
    }

    #[test]
    fn parse_date_invalid() {
        assert!(parse_date("not-a-date").is_none());
        assert!(parse_date("2026-13-01").is_none());
        assert!(parse_date("2026-00-01").is_none());
    }

    #[test]
    fn days_between_same_day() {
        assert_eq!(days_between((2026, 5, 15), (2026, 5, 15)), 0);
    }

    #[test]
    fn days_between_one_day() {
        assert_eq!(days_between((2026, 5, 15), (2026, 5, 16)), 1);
    }

    #[test]
    fn days_between_cross_month() {
        assert_eq!(days_between((2026, 1, 31), (2026, 2, 1)), 1);
    }

    #[test]
    fn next_day_basic() {
        assert_eq!(next_day(2026, 5, 15), (2026, 5, 16));
        assert_eq!(next_day(2026, 5, 31), (2026, 6, 1));
        assert_eq!(next_day(2026, 12, 31), (2027, 1, 1));
        assert_eq!(next_day(2024, 2, 28), (2024, 2, 29)); // leap year
        assert_eq!(next_day(2025, 2, 28), (2025, 3, 1)); // non-leap
    }

    #[test]
    fn prev_day_basic() {
        assert_eq!(prev_day(2026, 5, 15), (2026, 5, 14));
        assert_eq!(prev_day(2026, 6, 1), (2026, 5, 31));
        assert_eq!(prev_day(2027, 1, 1), (2026, 12, 31));
    }

    #[test]
    fn activity_score_all_today() {
        let mut props = HashMap::new();
        props.insert("last-accessed".to_string(), "2026-05-15".to_string());
        props.insert("last-modified".to_string(), "2026-05-15".to_string());
        props.insert("last-linked".to_string(), "2026-05-15".to_string());
        let w = ActivityWeights::default();
        let score = activity_score(&props, &w, (2026, 5, 15));
        // All age=0, so score = 1.0 + 2.0 + 0.5 = 3.5
        assert!((score - 3.5).abs() < 0.001);
    }

    #[test]
    fn activity_score_decays_with_age() {
        let mut props = HashMap::new();
        props.insert("last-accessed".to_string(), "2026-01-15".to_string());
        let w = ActivityWeights::default();
        let score = activity_score(&props, &w, (2026, 5, 15));
        // 120 days ago: 1.0 / (1 + 0.01 * 120) = 1.0 / 2.2 ≈ 0.4545
        assert!(score > 0.4 && score < 0.5, "score was {score}");
    }

    #[test]
    fn activity_score_empty_props() {
        let props = HashMap::new();
        let w = ActivityWeights::default();
        assert_eq!(activity_score(&props, &w, (2026, 5, 15)), 0.0);
    }

    #[test]
    fn body_hash_changes_on_content_change() {
        let content1 = ":PROPERTIES:\n:ID: abc\n:END:\nHello world\n";
        let content2 = ":PROPERTIES:\n:ID: abc\n:END:\nHello world!\n";
        assert_ne!(body_hash(content1), body_hash(content2));
    }

    #[test]
    fn body_hash_ignores_property_changes() {
        let content1 = ":PROPERTIES:\n:ID: abc\n:hash: old\n:END:\nBody\n";
        let content2 = ":PROPERTIES:\n:ID: abc\n:hash: new\n:END:\nBody\n";
        assert_eq!(body_hash(content1), body_hash(content2));
    }
}
