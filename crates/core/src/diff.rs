//! Minimal unified diff computation for AI propose_changes display.
//!
//! Produces colored diff lines (`+`, `-`, context) from old and new content.
//! Uses a simple LCS (longest common subsequence) algorithm — O(nm) but
//! sufficient for the file sizes AI edits typically produce.

/// A single line in a unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Unchanged context line.
    Context(String),
    /// Line added in the new version.
    Added(String),
    /// Line removed from the old version.
    Removed(String),
    /// Hunk header: @@ -old_start,old_count +new_start,new_count @@
    HunkHeader {
        old_start: usize,
        old_count: usize,
        new_start: usize,
        new_count: usize,
    },
}

/// Compute a unified diff between old and new content.
///
/// Returns a list of `DiffLine` entries with context lines around changes.
/// `context_lines` controls how many unchanged lines surround each hunk
/// (default: 3, matching standard unified diff).
pub fn unified_diff(old: &str, new: &str, context_lines: usize) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let lcs = lcs_table(&old_lines, &new_lines);
    let raw = build_diff_ops(&lcs, &old_lines, &new_lines);

    // Group into hunks with context.
    build_hunks(&raw, &old_lines, &new_lines, context_lines)
}

/// Render a unified diff as a string (standard format).
pub fn unified_diff_string(
    old: &str,
    new: &str,
    old_path: &str,
    new_path: &str,
    context_lines: usize,
) -> String {
    let lines = unified_diff(old, new, context_lines);
    if lines.is_empty() {
        return format!("--- a/{}\n+++ b/{}\n(no changes)\n", old_path, new_path);
    }
    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n", old_path));
    out.push_str(&format!("+++ b/{}\n", new_path));
    for line in &lines {
        match line {
            DiffLine::HunkHeader {
                old_start,
                old_count,
                new_start,
                new_count,
            } => {
                out.push_str(&format!(
                    "@@ -{},{} +{},{} @@\n",
                    old_start, old_count, new_start, new_count
                ));
            }
            DiffLine::Context(s) => {
                out.push_str(&format!(" {}\n", s));
            }
            DiffLine::Added(s) => {
                out.push_str(&format!("+{}\n", s));
            }
            DiffLine::Removed(s) => {
                out.push_str(&format!("-{}\n", s));
            }
        }
    }
    out
}

/// Raw diff operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Equal,
    Insert,
    Delete,
}

/// Build LCS length table (O(nm)).
fn lcs_table(old: &[&str], new: &[&str]) -> Vec<Vec<usize>> {
    let m = old.len();
    let n = new.len();
    let mut table = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                table[i][j] = table[i - 1][j - 1] + 1;
            } else {
                table[i][j] = table[i - 1][j].max(table[i][j - 1]);
            }
        }
    }
    table
}

/// Backtrack the LCS table to produce a sequence of diff ops.
fn build_diff_ops(table: &[Vec<usize>], old: &[&str], new: &[&str]) -> Vec<(DiffOp, usize, usize)> {
    let mut ops = Vec::new();
    let mut i = old.len();
    let mut j = new.len();

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            ops.push((DiffOp::Equal, i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            ops.push((DiffOp::Insert, 0, j - 1));
            j -= 1;
        } else if i > 0 {
            ops.push((DiffOp::Delete, i - 1, 0));
            i -= 1;
        }
    }
    ops.reverse();
    ops
}

/// Group diff ops into hunks with context lines.
fn build_hunks(
    ops: &[(DiffOp, usize, usize)],
    old: &[&str],
    new: &[&str],
    context: usize,
) -> Vec<DiffLine> {
    if ops.is_empty() {
        return Vec::new();
    }

    // Find ranges of change (non-Equal ops).
    let mut changes: Vec<(usize, usize)> = Vec::new(); // (start_idx, end_idx) in ops
    let mut i = 0;
    while i < ops.len() {
        if ops[i].0 != DiffOp::Equal {
            let start = i;
            while i < ops.len() && ops[i].0 != DiffOp::Equal {
                i += 1;
            }
            changes.push((start, i));
        } else {
            i += 1;
        }
    }

    if changes.is_empty() {
        return Vec::new();
    }

    // Merge nearby changes into hunks.
    let mut hunks: Vec<(usize, usize)> = Vec::new(); // (first_change_idx, last_change_idx) in changes[]
    let mut cur_start = 0;
    for ci in 1..changes.len() {
        // Count equal lines between this change and the previous one.
        let gap = changes[ci].0 - changes[ci - 1].1;
        if gap <= context * 2 {
            // Merge into current hunk.
            continue;
        }
        hunks.push((cur_start, ci - 1));
        cur_start = ci;
    }
    hunks.push((cur_start, changes.len() - 1));

    let mut result = Vec::new();

    for &(hunk_first, hunk_last) in &hunks {
        let change_start = changes[hunk_first].0;
        let change_end = changes[hunk_last].1;

        // Context before first change.
        let ctx_before = change_start.min(context);
        let ops_start = change_start - ctx_before;
        // Context after last change.
        let ctx_after = (ops.len() - change_end).min(context);
        let ops_end = change_end + ctx_after;

        // Compute hunk header line numbers from ops in range.
        let mut old_count = 0;
        let mut new_count = 0;
        let first_old = ops[ops_start..ops_end]
            .iter()
            .find(|(op, _, _)| *op == DiffOp::Equal || *op == DiffOp::Delete)
            .map(|(_, oi, _)| oi + 1)
            .unwrap_or(1);
        let first_new = ops[ops_start..ops_end]
            .iter()
            .find(|(op, _, _)| *op == DiffOp::Equal || *op == DiffOp::Insert)
            .map(|(_, _, ni)| ni + 1)
            .unwrap_or(1);

        for &(op, _, _) in &ops[ops_start..ops_end] {
            match op {
                DiffOp::Equal => {
                    old_count += 1;
                    new_count += 1;
                }
                DiffOp::Delete => old_count += 1,
                DiffOp::Insert => new_count += 1,
            }
        }

        result.push(DiffLine::HunkHeader {
            old_start: first_old,
            old_count,
            new_start: first_new,
            new_count,
        });

        for &(op, oi, ni) in &ops[ops_start..ops_end] {
            match op {
                DiffOp::Equal => result.push(DiffLine::Context(old[oi].to_string())),
                DiffOp::Delete => result.push(DiffLine::Removed(old[oi].to_string())),
                DiffOp::Insert => result.push(DiffLine::Added(new[ni].to_string())),
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_produces_no_diff() {
        let text = "line1\nline2\nline3\n";
        let diff = unified_diff(text, text, 3);
        assert!(diff.is_empty());
    }

    #[test]
    fn single_line_addition() {
        let old = "line1\nline3\n";
        let new = "line1\nline2\nline3\n";
        let diff = unified_diff(old, new, 3);
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Added(s) if s == "line2")));
    }

    #[test]
    fn single_line_removal() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline3\n";
        let diff = unified_diff(old, new, 3);
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Removed(s) if s == "line2")));
    }

    #[test]
    fn modification_shows_remove_and_add() {
        let old = "hello\n";
        let new = "world\n";
        let diff = unified_diff(old, new, 3);
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Removed(s) if s == "hello")));
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Added(s) if s == "world")));
    }

    #[test]
    fn empty_to_content() {
        let diff = unified_diff("", "new content\n", 3);
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Added(s) if s == "new content")));
    }

    #[test]
    fn content_to_empty() {
        let diff = unified_diff("old content\n", "", 3);
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Removed(s) if s == "old content")));
    }

    #[test]
    fn unified_diff_string_format() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline2_modified\nline3\n";
        let s = unified_diff_string(old, new, "test.rs", "test.rs", 3);
        assert!(s.contains("--- a/test.rs"));
        assert!(s.contains("+++ b/test.rs"));
        assert!(s.contains("@@"));
        assert!(s.contains("-line2"));
        assert!(s.contains("+line2_modified"));
    }

    #[test]
    fn context_lines_included() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\n";
        let diff = unified_diff(old, new, 1);
        // Should have context: b before, d after
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Context(s) if s == "b")));
        assert!(diff
            .iter()
            .any(|d| matches!(d, DiffLine::Context(s) if s == "d")));
    }
}
