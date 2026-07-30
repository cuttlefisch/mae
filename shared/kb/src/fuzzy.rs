//! Fuzzy matching algorithm shared by KB search and file picker.
//!
//! Extracted here so both `mae-kb` (KB search fallback) and `mae-core`
//! (file picker, command palette) can use it without circular deps.

/// Normalize separator characters: space and underscore become hyphen,
/// and backslash becomes forward slash. The backslash mapping matters on
/// Windows, where `FilePicker::scan` yields OS-native paths (`src\main.rs`)
/// -- every boundary/basename check below keys off `/` specifically, so
/// without this a Windows path never hits the basename or boundary-aligned
/// tiers. See cuttlefisch/mae#534.
fn normalize_sep(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            '\\' => '/',
            o => o,
        })
        .collect()
}

/// Tiered fuzzy scoring for a query against a candidate string.
///
/// Returns `None` if the query is not a subsequence of the candidate.
///
/// Tiers (highest score first):
/// 1. Exact equality
/// 2. Suffix match
/// 3. Contiguous substring
/// 4. Fuzzy subsequence with word-boundary bonuses
pub fn score_match(path: &str, query: &[char]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let path_lower = normalize_sep(&path.to_lowercase());
    let query_str: String = normalize_sep(&query.iter().collect::<String>());
    let path_len = path.len() as i64;

    // ---- Tier 1: exact equality ----
    if path_lower == query_str {
        return Some(1_000_000);
    }

    // ---- Tier 1.5: query exactly matches the basename ----
    let basename_start = path_lower.rfind('/').map(|p| p + 1).unwrap_or(0);
    let basename = &path_lower[basename_start..];
    if basename == query_str {
        return Some(750_000 - path_len);
    }

    // ---- Tier 2/3: suffix match ----
    if path_lower.ends_with(&query_str) && path_lower.len() > query_str.len() {
        let rest_len = path_lower.len() - query_str.len();
        let boundary_aligned = path_lower.as_bytes()[rest_len - 1] == b'/';
        let base = if boundary_aligned { 500_000 } else { 100_000 };
        return Some(base - path_len);
    }

    // ---- Tier 4: contiguous substring ----
    if let Some(pos) = path_lower.find(&query_str) {
        let boundary_aligned = pos == 0
            || matches!(
                path_lower.as_bytes().get(pos - 1),
                Some(b'/' | b'.' | b'_' | b'-')
            );
        let base = if boundary_aligned { 50_000 } else { 10_000 };
        let last_slash = path_lower.rfind('/').map(|p| p + 1).unwrap_or(0);
        let filename_bonus = if pos >= last_slash { 1_000 } else { 0 };
        return Some(base + filename_bonus - path_len);
    }

    // ---- Tier 5: fuzzy subsequence ----
    if query_str.contains('/') {
        return None;
    }

    let path_chars: Vec<char> = path_lower.chars().collect();
    // Reuse `query_str` (already ran through `normalize_sep`) rather than
    // re-deriving from `query` with a second, easily-divergent copy of the
    // same normalization -- this is what caused #534's Tier-5 half of the
    // bug (the duplicate closure here mapped space/underscore but not `\`).
    let query_chars: Vec<char> = query_str.chars().collect();
    let mut qi = 0;
    let mut score: i64 = 0;
    let mut last_match_pos: Option<usize> = None;
    let mut first_match_pos: Option<usize> = None;

    for (pi, &pc) in path_chars.iter().enumerate() {
        if qi < query_chars.len() && pc == query_chars[qi] {
            if first_match_pos.is_none() {
                first_match_pos = Some(pi);
            }
            if let Some(last) = last_match_pos {
                if pi == last + 1 {
                    score += 10;
                }
            }
            if pi == 0
                || matches!(
                    path_chars.get(pi.saturating_sub(1)),
                    Some('/' | '.' | '_' | '-')
                )
            {
                score += 8;
            }
            let last_slash = path_chars.iter().rposition(|c| *c == '/').unwrap_or(0);
            if pi >= last_slash {
                score += 5;
            }
            last_match_pos = Some(pi);
            qi += 1;
        }
    }

    if qi < query_chars.len() {
        return None;
    }

    score -= path_len / 4;

    if let Some(fp) = first_match_pos {
        let last_slash = path_chars
            .iter()
            .rposition(|c| *c == '/')
            .map(|p| p + 1)
            .unwrap_or(0);
        if fp == last_slash {
            score += 15;
        }
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_highest() {
        let q: Vec<char> = "buffer".chars().collect();
        assert!(score_match("buffer", &q).unwrap() > score_match("buffer-mode", &q).unwrap());
    }

    #[test]
    fn substring_match() {
        let q: Vec<char> = "module".chars().collect();
        assert!(score_match("concept:modules", &q).is_some());
    }

    #[test]
    fn fuzzy_subsequence() {
        let q: Vec<char> = "sb".chars().collect();
        assert!(score_match("switch-buffer", &q).is_some());
    }

    #[test]
    fn no_match_returns_none() {
        let q: Vec<char> = "xyz".chars().collect();
        assert!(score_match("buffer", &q).is_none());
    }

    #[test]
    fn empty_query() {
        assert_eq!(score_match("anything", &[]), Some(0));
    }

    #[test]
    fn separator_space_matches_hyphen() {
        let q: Vec<char> = "kb daily".chars().collect();
        assert!(
            score_match("kb-daily", &q).is_some(),
            "space should match hyphen"
        );
    }

    #[test]
    fn separator_space_matches_in_namespaced_id() {
        let q: Vec<char> = "window groups".chars().collect();
        assert!(
            score_match("concept:window-groups", &q).is_some(),
            "space should match hyphen in namespaced ID"
        );
    }

    #[test]
    fn separator_underscore_matches_hyphen() {
        let q: Vec<char> = "kb_daily".chars().collect();
        assert!(
            score_match("kb-daily", &q).is_some(),
            "underscore should match hyphen"
        );
    }

    // Windows-path adversarial coverage (cuttlefisch/mae#534). The oracle
    // throughout is exact score equality against the equivalent `/`-path,
    // not just `is_some()` -- a weaker check would pass even if the
    // Windows path fell into a lower-scoring tier, which is exactly the
    // bug that shipped unnoticed.

    #[test]
    fn windows_path_basename_exact_match_same_tier_as_unix() {
        // Tier 1.5: query exactly equals the basename.
        let q: Vec<char> = "main.rs".chars().collect();
        let unix = score_match("src/main.rs", &q);
        let windows = score_match("src\\main.rs", &q);
        assert_eq!(unix, windows, "unix: {unix:?}, windows: {windows:?}");
        assert!(windows.unwrap() >= 750_000 - 11, "expected tier 1.5 score");
    }

    #[test]
    fn windows_path_boundary_aligned_match_same_tier_as_unix() {
        // Tier 4: a contiguous-substring match right after a path
        // separator should be boundary-aligned (50,000 base) regardless
        // of which separator byte the platform used.
        let q: Vec<char> = "config".chars().collect();
        let unix = score_match("crates/core/config.rs", &q);
        let windows = score_match("crates\\core\\config.rs", &q);
        assert_eq!(unix, windows, "unix: {unix:?}, windows: {windows:?}");
        assert!(
            windows.unwrap() >= 50_000 - 22,
            "expected boundary-aligned tier"
        );
    }

    #[test]
    fn windows_path_filename_bonus_same_tier_as_unix() {
        // Tier 4's filename_bonus: a match inside the final path segment
        // should score the same whether that segment was split off by
        // `/` or `\`.
        let q: Vec<char> = "options".chars().collect();
        let unix = score_match("crates/core/src/options.rs", &q);
        let windows = score_match("crates\\core\\src\\options.rs", &q);
        assert_eq!(unix, windows, "unix: {unix:?}, windows: {windows:?}");
    }

    #[test]
    fn windows_path_fuzzy_subsequence_same_score_as_unix() {
        // Tier 5: the fuzzy subsequence pass also keys off `/` for its
        // own boundary/last-segment bonuses (`path_chars`/`query_chars`
        // both go through `normalize_sep`/`query_str` now) -- use a query
        // that only subsequence-matches, not substring-matches, so this
        // exercises tier 5 specifically rather than tier 4.
        let q: Vec<char> = "cfpr".chars().collect();
        let unix = score_match("crates/core/src/file_picker.rs", &q);
        let windows = score_match("crates\\core\\src\\file_picker.rs", &q);
        assert_eq!(unix, windows, "unix: {unix:?}, windows: {windows:?}");
        assert!(unix.is_some(), "expected a tier-5 fuzzy match, got None");
    }

    #[test]
    fn windows_path_no_spurious_match_stays_none_on_both() {
        // Negative case: a query that shouldn't match at all must stay
        // `None` on both separator styles -- guards against a fix that
        // over-normalizes and starts matching things it shouldn't.
        let q: Vec<char> = "zzz-nonexistent".chars().collect();
        assert_eq!(score_match("src/main.rs", &q), None);
        assert_eq!(score_match("src\\main.rs", &q), None);
    }
}
