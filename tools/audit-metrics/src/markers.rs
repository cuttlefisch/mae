//! `@ai-caution` / `@stability` inventory and the 3-way cross-reference check.
//!
//! CLAUDE.md's "Debt/Invariant Tagging" section defines a discipline: an
//! `@ai-caution: [architecture-debt]` marker in code must be cross-linked from
//! `ROADMAP.md`'s "Architecture Debt" section and — for size-ceiling debt —
//! from `docs/AUDIT_BASELINE.json`, so a reader landing in any one of the three
//! finds the others. That discipline was maintained by hand and had holes: an
//! orphaned marker, one pointing at an already-closed issue, and two missing
//! their `[category]` tag. This module makes the check mechanical.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Caution {
    pub path: String,
    pub line: usize,
    /// `None` when the marker omitted its `[category]` bracket — itself a
    /// convention violation, since categories are what make markers greppable
    /// as a group.
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StabilityMarker {
    pub path: String,
    pub value: String,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MarkerReport {
    pub cautions: Vec<Caution>,
    pub stability: Vec<StabilityMarker>,
    /// `@ai-caution: [architecture-debt]` markers whose file is referenced by
    /// none of ROADMAP.md, mae-audit.md, or the accepted-exceptions baseline.
    pub orphaned_debt_markers: Vec<String>,
    /// Files listed as tracked exceptions in mae-audit.md that carry no
    /// in-code marker — the reverse orphan.
    pub untracked_in_code: Vec<String>,
    /// Markers written without a `[category]` bracket.
    pub uncategorised: Vec<String>,
    /// Crate roots (`lib.rs`/`main.rs`) with no `@stability:` marker.
    pub missing_stability: Vec<String>,
}

/// Extract the `[category]` from an `@ai-caution:` line, if present.
fn parse_category(line: &str) -> Option<String> {
    let after = line.split("@ai-caution:").nth(1)?.trim_start();
    let rest = after.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].to_string())
}

fn parse_stability(line: &str) -> Option<String> {
    let after = line.split("@stability:").nth(1)?.trim();
    // Take the first whitespace-delimited token: `stable`, `experimental`,
    // `unstable`, possibly followed by a parenthetical.
    after.split_whitespace().next().map(|s| s.to_string())
}

/// Build-support binaries under `tools/` are excluded from the marker scan:
/// they are not shipped crates, and this tool's own source necessarily *names*
/// the marker strings while describing them, which would otherwise report the
/// scanner as its own violation.
fn scanned_for_markers(path: &str) -> bool {
    !path.starts_with("tools/")
}

/// Scan already-loaded sources for markers. Takes the same `(rel_path, source)`
/// pairs `scan::collect_files` produced, so files are read once for both passes.
pub fn scan(files: &[(String, String)]) -> MarkerReport {
    let mut report = MarkerReport::default();

    for (path, src) in files.iter().filter(|(p, _)| scanned_for_markers(p)) {
        for (i, line) in src.lines().enumerate() {
            if line.contains("@ai-caution:") {
                let category = parse_category(line);
                if category.is_none() {
                    report.uncategorised.push(format!("{path}:{}", i + 1));
                }
                report.cautions.push(Caution {
                    path: path.clone(),
                    line: i + 1,
                    category,
                });
            }
            if line.contains("@stability:") {
                if let Some(value) = parse_stability(line) {
                    report.stability.push(StabilityMarker {
                        path: path.clone(),
                        value,
                    });
                }
            }
        }
    }

    for (path, src) in files.iter().filter(|(p, _)| scanned_for_markers(p)) {
        if is_crate_root(path) && !src.contains("@stability:") {
            report.missing_stability.push(path.clone());
        }
    }

    report.cautions.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    report.stability.sort_by(|a, b| a.path.cmp(&b.path));
    report.missing_stability.sort();
    report.uncategorised.sort();
    report
}

/// A crate root that should carry a crate-level `@stability:` marker.
/// `tools/` are build-support binaries, not shipped crates.
fn is_crate_root(path: &str) -> bool {
    (path.ends_with("/src/lib.rs") || path.ends_with("/src/main.rs"))
        && !path.starts_with("tools/")
}

/// Cross-check `[architecture-debt]` markers against the tracking surfaces.
///
/// A marker counts as tracked if it is cited by `ROADMAP.md`, by
/// `.claude/commands/mae-audit.md`, **or** if the file is in
/// `docs/AUDIT_BASELINE.json` (`baselined`). The baseline is the machine-checked
/// third leg of CLAUDE.md's cross-reference discipline — requiring prose to
/// restate what the baseline already holds is exactly the duplication that let
/// the old numbers drift.
pub fn cross_reference(
    report: &mut MarkerReport,
    roadmap: &str,
    audit_doc: &str,
    baselined: &[String],
) {
    let debt_files: Vec<String> = report
        .cautions
        .iter()
        .filter(|c| c.category.as_deref() == Some("architecture-debt"))
        .map(|c| c.path.clone())
        .collect();

    for path in &debt_files {
        // Docs cite paths inconsistently (full path, or just the file name).
        // Accept either — this check is about discoverability, not formatting.
        let base = path.rsplit('/').next().unwrap_or(path);
        let cited = |doc: &str| doc.contains(path.as_str()) || doc.contains(base);
        if !cited(roadmap)
            && !cited(audit_doc)
            && !baselined.contains(path)
            && !report.orphaned_debt_markers.contains(path)
        {
            report.orphaned_debt_markers.push(path.clone());
        }
    }

    // Reverse direction: a path the audit doc lists as a tracked exception
    // but which carries no in-code marker at all.
    for line in audit_doc.lines() {
        let Some(path) = extract_backticked_rs_path(line) else {
            continue;
        };
        let has_marker = report.cautions.iter().any(|c| c.path == path);
        if !has_marker && !report.untracked_in_code.contains(&path) {
            report.untracked_in_code.push(path);
        }
    }

    report.orphaned_debt_markers.sort();
    report.untracked_in_code.sort();
}

/// Pull a `` `crates/foo/src/bar.rs` ``-style path out of a doc bullet.
fn extract_backticked_rs_path(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    // Only list items -- prose mentions shouldn't count as tracking.
    if !trimmed.starts_with("- ") {
        return None;
    }
    let start = trimmed.find('`')? + 1;
    let rest = &trimmed[start..];
    let end = rest.find('`')?;
    let candidate = &rest[..end];
    // A glob (`crates/*/src/lib.rs`) is a class of files, not a tracked
    // exception -- it can't be checked for a marker and isn't one.
    (candidate.ends_with(".rs") && candidate.contains('/') && !candidate.contains('*'))
        .then(|| candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<(String, String)> {
        vec![
            (
                "crates/core/src/big.rs".to_string(),
                "//! @ai-caution: [architecture-debt] huge file\nfn a() {}\n".to_string(),
            ),
            (
                "crates/core/src/lib.rs".to_string(),
                "//! @stability: stable\n".to_string(),
            ),
            (
                "shared/mcp/src/main.rs".to_string(),
                "// @ai-caution: no bracket here\n".to_string(),
            ),
        ]
    }

    #[test]
    fn categorised_and_uncategorised_markers_are_distinguished() {
        let r = scan(&files());
        assert_eq!(r.cautions.len(), 2);
        assert_eq!(
            r.cautions[0].category.as_deref(),
            Some("architecture-debt")
        );
        assert_eq!(r.uncategorised, vec!["shared/mcp/src/main.rs:1"]);
    }

    #[test]
    fn crate_root_without_stability_is_reported() {
        let r = scan(&files());
        // core/src/lib.rs has one; mcp/src/main.rs does not.
        assert_eq!(r.missing_stability, vec!["shared/mcp/src/main.rs"]);
    }

    #[test]
    fn a_debt_marker_cited_by_neither_doc_is_orphaned() {
        let mut r = scan(&files());
        cross_reference(&mut r, "no mention", "no mention", &[]);
        assert_eq!(r.orphaned_debt_markers, vec!["crates/core/src/big.rs"]);
    }

    #[test]
    fn a_debt_marker_cited_by_either_doc_is_not_orphaned() {
        // Citing by bare file name counts -- the docs do this inconsistently
        // and the check is about discoverability, not citation style.
        for (roadmap, audit) in [("see big.rs", "none"), ("none", "`crates/core/src/big.rs`")] {
            let mut r = scan(&files());
            cross_reference(&mut r, roadmap, audit, &[]);
            assert!(
                r.orphaned_debt_markers.is_empty(),
                "roadmap={roadmap:?} audit={audit:?} -> {:?}",
                r.orphaned_debt_markers
            );
        }
    }

    #[test]
    fn a_debt_marker_present_in_the_baseline_is_tracked() {
        // The baseline is the machine-checked third leg -- prose need not
        // restate what it already holds.
        let mut r = scan(&files());
        cross_reference(&mut r, "", "", &["crates/core/src/big.rs".to_string()]);
        assert!(r.orphaned_debt_markers.is_empty(), "{:?}", r.orphaned_debt_markers);
    }

    #[test]
    fn audit_doc_entry_with_no_in_code_marker_is_reported() {
        let mut r = scan(&files());
        cross_reference(&mut r, "", "- `crates/core/src/ghost.rs` — 900 lines\n", &[]);
        assert!(r.untracked_in_code.contains(&"crates/core/src/ghost.rs".to_string()));
    }

    #[test]
    fn prose_mentions_do_not_count_as_tracking() {
        // Only `- ` list items are treated as tracking entries.
        assert_eq!(extract_backticked_rs_path("see `crates/a/src/b.rs` for detail"), None);
        assert_eq!(
            extract_backticked_rs_path("- `crates/a/src/b.rs` — 900 lines"),
            Some("crates/a/src/b.rs".to_string())
        );
        // A backticked non-path must not be mistaken for one.
        assert_eq!(extract_backticked_rs_path("- `make ci` runs tests"), None);
        // Nor a glob -- it names a class of files, not one tracked exception.
        assert_eq!(extract_backticked_rs_path("- `crates/*/src/lib.rs` markers"), None);
    }

    #[test]
    fn the_scanner_never_reports_its_own_source() {
        // This file necessarily contains the literal marker strings while
        // describing them; without the tools/ exclusion it would flag itself.
        let files = vec![(
            "tools/audit-metrics/src/markers.rs".to_string(),
            "//! describes @ai-caution: [architecture-debt] markers\n".to_string(),
        )];
        let r = scan(&files);
        assert!(r.cautions.is_empty(), "{:?}", r.cautions);
        assert!(r.uncategorised.is_empty());
    }
}
