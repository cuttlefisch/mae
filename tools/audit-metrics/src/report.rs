//! Baseline comparison and report rendering.
//!
//! The baseline is what makes this a *ratchet* rather than a wall of 126
//! pre-existing failures. Accepted debt is recorded with the size it was
//! accepted at; the gate then fails on genuinely new debt, or on accepted debt
//! that grew — which is exactly the failure mode the hand-maintained prose
//! missed (one tracked file grew +96% while its documented number sat still).

use crate::scan::FileMetrics;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much an accepted exception may grow before the gate fails it.
/// Generous enough that ordinary maintenance inside a big file doesn't trip
/// the build; tight enough that a file cannot silently double.
pub const GROWTH_TOLERANCE: f64 = 0.10;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    /// Why this file is documented here rather than fixed. Written for a human
    /// reading a CI failure, not for the tool.
    #[serde(default)]
    pub note: String,
    /// path -> accepted line count at the time it was baselined.
    #[serde(default)]
    pub accepted: BTreeMap<String, usize>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Over ceiling, not in the baseline — brand-new debt.
    NewViolation { path: String, lines: usize, ceiling: usize },
    /// In the baseline, but grew past `GROWTH_TOLERANCE`.
    Grew { path: String, was: usize, now: usize },
    /// In the baseline but no longer over ceiling — the entry can be dropped.
    Resolved { path: String },
}

impl Verdict {
    /// Only the first two block CI. `Resolved` is good news and is reported
    /// as guidance, never as a failure — otherwise fixing a file would break
    /// the build, which would teach exactly the wrong lesson.
    pub fn is_failure(&self) -> bool {
        !matches!(self, Verdict::Resolved { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Verdict::NewViolation { path, lines, ceiling } => {
                format!("NEW  {path} — {lines} lines, ceiling {ceiling}")
            }
            Verdict::Grew { path, was, now } => {
                let pct = ((*now as f64 / *was as f64) - 1.0) * 100.0;
                format!("GREW {path} — {was} → {now} (+{pct:.0}%)")
            }
            Verdict::Resolved { path } => {
                format!("FIXED {path} — now under ceiling, drop it from the baseline")
            }
        }
    }
}

pub fn compare(metrics: &[FileMetrics], baseline: &Baseline) -> Vec<Verdict> {
    let mut out = Vec::new();

    for m in metrics {
        match baseline.accepted.get(&m.path) {
            Some(&was) if m.over_ceiling() => {
                let limit = (was as f64 * (1.0 + GROWTH_TOLERANCE)) as usize;
                if m.lines > limit {
                    out.push(Verdict::Grew {
                        path: m.path.clone(),
                        was,
                        now: m.lines,
                    });
                }
            }
            Some(_) => out.push(Verdict::Resolved { path: m.path.clone() }),
            None if m.over_ceiling() => out.push(Verdict::NewViolation {
                path: m.path.clone(),
                lines: m.lines,
                ceiling: m.ceiling(),
            }),
            None => {}
        }
    }

    out.sort_by_key(|v| match v {
        Verdict::NewViolation { path, .. } | Verdict::Grew { path, .. } | Verdict::Resolved { path } => path.clone(),
    });
    out
}

/// Build a fresh baseline from current metrics — every over-ceiling file
/// accepted at its present size. Used by `--bless`.
pub fn bless(metrics: &[FileMetrics], note: &str) -> Baseline {
    Baseline {
        note: note.to_string(),
        accepted: metrics
            .iter()
            .filter(|m| m.over_ceiling())
            .map(|m| (m.path.clone(), m.lines))
            .collect(),
    }
}

/// Short human summary printed after every run.
pub fn summarise(metrics: &[FileMetrics]) -> String {
    let total_lines: usize = metrics.iter().map(|m| m.lines).sum();
    let code: usize = metrics.iter().map(|m| m.code_lines).sum();
    let tests: usize = metrics.iter().map(|m| m.test_lines).sum();
    let over = metrics.iter().filter(|m| m.over_ceiling()).count();
    let fn_over = metrics
        .iter()
        .filter(|m| m.max_fn_lines > crate::scan::FUNCTION_CEILING)
        .count();
    let struct_over = metrics
        .iter()
        .filter(|m| m.max_struct_fields > crate::scan::STRUCT_FIELD_CEILING)
        .count();
    let match_over = metrics
        .iter()
        .filter(|m| m.max_match_arms > crate::scan::MATCH_ARM_CEILING)
        .count();
    let nest_over = metrics
        .iter()
        .filter(|m| m.max_nesting > crate::scan::NESTING_CEILING)
        .count();
    let dominated = metrics.iter().filter(|m| m.inline_tests_dominate()).count();
    let unparsed = metrics.iter().filter(|m| m.parse_failed).count();
    let test_fns: usize = metrics.iter().map(|m| m.test_count).sum();

    let mut s = format!(
        "  {} files, {total_lines} lines ({code} code / {tests} test), {test_fns} test fns\n\
         \x20 over file ceiling: {over}   fn>{}: {fn_over}   struct>{}: {struct_over}   \
         match>{}: {match_over}   nesting>{}: {nest_over}\n\
         \x20 inline tests dominate (>50%): {dominated}",
        metrics.len(),
        crate::scan::FUNCTION_CEILING,
        crate::scan::STRUCT_FIELD_CEILING,
        crate::scan::MATCH_ARM_CEILING,
        crate::scan::NESTING_CEILING,
    );
    if unparsed > 0 {
        s.push_str(&format!("\n  UNPARSEABLE (metrics unreliable): {unparsed}"));
    }
    s
}

/// The files breaching the most ceilings at once, worst first. Informational:
/// the CI gate ratchets on file size only (the non-size ceilings have a large
/// pre-existing population, and failing all of them at once would be an
/// unactionable wall rather than a ratchet). Surfacing them here is what makes
/// them visible at all — nothing enforced or reported them before.
pub fn worst_offenders(metrics: &[FileMetrics], limit: usize) -> Vec<String> {
    let mut ranked: Vec<(usize, usize, &FileMetrics)> = metrics
        .iter()
        .map(|m| (m.violations().len(), m.lines, m))
        .filter(|(n, _, _)| *n > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, m)| format!("{} — {}", m.path, m.violations().join(", ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(path: &str, lines: usize) -> FileMetrics {
        let mut m = crate::scan::measure(path, "fn a() {}\n");
        m.lines = lines;
        m
    }

    fn baseline_with(path: &str, lines: usize) -> Baseline {
        let mut b = Baseline::default();
        b.accepted.insert(path.to_string(), lines);
        b
    }

    #[test]
    fn a_new_over_ceiling_file_fails() {
        let v = compare(&[metric("crates/a/src/new.rs", 900)], &Baseline::default());
        assert_eq!(
            v,
            vec![Verdict::NewViolation {
                path: "crates/a/src/new.rs".into(),
                lines: 900,
                ceiling: 800
            }]
        );
        assert!(v[0].is_failure());
    }

    #[test]
    fn an_accepted_file_holding_steady_passes() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 3000)],
            &baseline_with("crates/a/src/big.rs", 3000),
        );
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn an_accepted_file_that_grew_past_tolerance_fails() {
        // +96% is the real graph_view_ops.rs case that drifted unnoticed.
        let v = compare(
            &[metric("crates/a/src/big.rs", 8745)],
            &baseline_with("crates/a/src/big.rs", 4464),
        );
        assert!(matches!(v.as_slice(), [Verdict::Grew { .. }]), "{v:?}");
        assert!(v[0].is_failure());
    }

    #[test]
    fn growth_within_tolerance_is_allowed() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 3100)],
            &baseline_with("crates/a/src/big.rs", 3000),
        );
        assert!(v.is_empty(), "5% growth should not fail the build: {v:?}");
    }

    #[test]
    fn shrinking_below_ceiling_is_reported_but_never_fails_the_build() {
        // Fixing a file must not break CI.
        let v = compare(
            &[metric("crates/a/src/big.rs", 400)],
            &baseline_with("crates/a/src/big.rs", 3000),
        );
        assert_eq!(v, vec![Verdict::Resolved { path: "crates/a/src/big.rs".into() }]);
        assert!(!v[0].is_failure());
    }

    #[test]
    fn test_files_are_judged_against_the_lower_ceiling() {
        // 600 lines is fine for source, over ceiling for a test file.
        assert!(compare(&[metric("crates/a/src/x.rs", 600)], &Baseline::default()).is_empty());
        let v = compare(&[metric("crates/a/tests/x.rs", 600)], &Baseline::default());
        assert!(matches!(v.as_slice(), [Verdict::NewViolation { ceiling: 500, .. }]), "{v:?}");
    }

    #[test]
    fn bless_accepts_exactly_the_over_ceiling_files() {
        let b = bless(
            &[metric("a/src/big.rs", 900), metric("a/src/small.rs", 10)],
            "note",
        );
        assert_eq!(b.accepted.len(), 1);
        assert_eq!(b.accepted.get("a/src/big.rs"), Some(&900));
    }
}
