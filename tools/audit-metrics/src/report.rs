//! Baseline comparison and report rendering.
//!
//! The baseline is what makes this a *ratchet* rather than a wall of
//! pre-existing failures. What it gates changed in 2026-08 — see below.
//!
//! # What is gated, and why it is no longer file size
//!
//! **File size is measured and reported, never gated.** It was gated until
//! 2026-08-25, and the evidence against that is in-repo and one-sided:
//!
//! * The gate's one real capability — stopping a brand-new oversized file —
//!   was exercised and lost. `crates/scheme/src/parity_tests.rs` was created
//!   AFTER the ratchet landed, at 896 lines (nearly 2x the 500-line test
//!   ceiling), and was **blessed into the baseline rather than split**.
//! * It grandfathered 141 violations, median 1,210 against an 800 ceiling,
//!   largest 14,516 — i.e. it froze the very debt it existed to prevent, and
//!   then charged everyone else for it.
//! * A **proportional** tolerance inverted the incentive: the 14,516-line file
//!   could still add 1,451 lines; a 571-line test file could add 11.
//! * Sub-threshold drift on `main` meant an unrelated PR adding ~20 lines wore
//!   a failure it did not cause — four times in the last forty CI runs,
//!   including on a **docs-only** PR.
//! * Every `bless` is an "effective false positive" in Google Tricorder's
//!   sense (*"any report where a user chooses not to take action"*): roughly 5
//!   blesses against 2 genuine refactors in 23 days, versus their stated bar
//!   for *blocking* checks of essentially zero.
//! * On this repo, `corr(churn, fix-commits) = 0.88` vs `corr(size, .) = 0.50`.
//!   Size is the weakest signal available here.
//!
//! Prior art agrees: PMD **deleted** `ExcessiveClassLength` (*"LoC is noisy"*),
//! Google's Tricorder excluded size/complexity metrics as unactionable, airbnb
//! sets ESLint's `max-lines` to `off`, and `checkpatch.pl` has no length check
//! at all.
//!
//! **Function length and nesting depth are gated instead.** They are per-item,
//! so they cannot drift, cannot be inherited from `main`, and are actionable in
//! the way Tricorder requires: *"the problem should be obvious and actionable
//! when pointed out."* "This 1,271-line function has a 78-arm match" is; "this
//! file is 1,154 lines" is not.
//!
//! **And the ratchet is monotonic on an exact value, with no tolerance band.**
//! That band was MAE's own invention — RuboCop todo files, PHPStan/Psalm/
//! Android-lint baselines, betterer and mypy-baseline are all exact — and it is
//! what produced the drift-then-blame failure.

use crate::scan::{FileMetrics, FUNCTION_CEILING, NESTING_CEILING};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Per-file structural debt that IS gated. Monotonic: neither value may rise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Structure {
    /// Longest function in the file, in lines.
    #[serde(default)]
    pub max_fn_lines: usize,
    /// Deepest block nesting in the file.
    #[serde(default)]
    pub max_nesting: usize,
}

impl Structure {
    fn of(m: &FileMetrics) -> Self {
        Self { max_fn_lines: m.max_fn_lines, max_nesting: m.max_nesting }
    }
    fn over_ceiling(&self) -> bool {
        self.max_fn_lines > FUNCTION_CEILING || self.max_nesting > NESTING_CEILING
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    /// Why this file is documented here rather than fixed. Written for a human
    /// reading a CI failure, not for the tool.
    #[serde(default)]
    pub note: String,
    /// path -> accepted line count. **Reported, not gated** — retained because
    /// drift *detection* is the mechanism's genuine, documented win: it replaced
    /// a hand-maintained prose list in which 14 of 15 figures had rotted, one by
    /// +96%.
    #[serde(default)]
    pub accepted: BTreeMap<String, usize>,
    /// path -> accepted structural debt. **This is what gates CI.**
    #[serde(default)]
    pub accepted_structure: BTreeMap<String, Structure>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Over a structural ceiling and not in the baseline — brand-new debt.
    NewViolation { path: String, what: String },
    /// In the baseline, and got worse. Exact, not proportional.
    Grew { path: String, what: String },
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
            Verdict::NewViolation { path, what } => format!("NEW  {path} — {what}"),
            Verdict::Grew { path, what } => format!("WORSE {path} — {what}"),
            Verdict::Resolved { path } => {
                format!("FIXED {path} — now under ceiling, drop it from the baseline")
            }
        }
    }
}

/// Describe how `now` breaches the ceilings, or how it worsened against `was`.
fn describe_structure(now: &Structure, was: Option<&Structure>) -> Option<String> {
    let mut parts = Vec::new();
    match was {
        // Baselined: an increase fails only if it is ALSO over the ceiling.
        // Exact — no tolerance band.
        //
        // The second half of that condition is not a softening; without it the
        // gate fails changes that are entirely fine. A file enters the baseline
        // because ONE metric breached, and every other metric on it would then be
        // frozen at whatever value it happened to hold — `remote_hub.rs` was
        // failed for nesting 2 → 3 against a ceiling of 4. That is precisely the
        // "fires on things that are not problems" noise this gate was rebuilt to
        // remove, and it would have taught contributors to reach for `bless`.
        //
        // Below the ceiling there is nothing to ratchet: the ceiling IS the
        // standard, and the baseline exists only to stop already-breaching files
        // getting worse.
        Some(was) => {
            if now.max_fn_lines > was.max_fn_lines && now.max_fn_lines > FUNCTION_CEILING {
                parts.push(format!(
                    "longest fn {} → {} (ceiling {FUNCTION_CEILING})",
                    was.max_fn_lines, now.max_fn_lines
                ));
            }
            if now.max_nesting > was.max_nesting && now.max_nesting > NESTING_CEILING {
                parts.push(format!(
                    "nesting {} → {} (ceiling {NESTING_CEILING})",
                    was.max_nesting, now.max_nesting
                ));
            }
        }
        // Not baselined: any breach is new debt.
        None => {
            if now.max_fn_lines > FUNCTION_CEILING {
                parts.push(format!("fn {}>{FUNCTION_CEILING} lines", now.max_fn_lines));
            }
            if now.max_nesting > NESTING_CEILING {
                parts.push(format!("nesting {}>{NESTING_CEILING}", now.max_nesting));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

pub fn compare(metrics: &[FileMetrics], baseline: &Baseline) -> Vec<Verdict> {
    let mut out = Vec::new();

    for m in metrics {
        // A file `syn` could not parse reports zeroes for exactly these fields,
        // which would read as "no debt" and, worse, as an improvement against a
        // baseline. Skip it — `scan` surfaces the parse failure separately.
        if m.parse_failed {
            continue;
        }
        let now = Structure::of(m);
        match baseline.accepted_structure.get(&m.path) {
            Some(was) => {
                if let Some(what) = describe_structure(&now, Some(was)) {
                    out.push(Verdict::Grew { path: m.path.clone(), what });
                } else if !now.over_ceiling() {
                    out.push(Verdict::Resolved { path: m.path.clone() });
                }
            }
            None => {
                if let Some(what) = describe_structure(&now, None) {
                    out.push(Verdict::NewViolation { path: m.path.clone(), what });
                }
            }
        }
    }
    out
}

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
/// Re-baseline. Records BOTH the gated structural debt and the reported line
/// counts, so the size report keeps working while only structure gates.
pub fn bless(metrics: &[FileMetrics], note: &str) -> Baseline {
    Baseline {
        note: note.to_string(),
        accepted: metrics
            .iter()
            .filter(|m| m.over_ceiling())
            .map(|m| (m.path.clone(), m.lines))
            .collect(),
        accepted_structure: metrics
            .iter()
            .filter(|m| !m.parse_failed)
            .map(|m| (m.path.clone(), Structure::of(m)))
            .filter(|(_, st)| st.over_ceiling())
            .collect(),
    }
}

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

    /// A file whose longest function is `fn_lines` and whose deepest nesting is
    /// `nesting`. `lines` is set independently so the tests can prove that file
    /// SIZE no longer influences the verdict.
    fn metric(path: &str, lines: usize, fn_lines: usize, nesting: usize) -> FileMetrics {
        let mut m = crate::scan::measure(path, "fn a() {}\n");
        m.lines = lines;
        m.max_fn_lines = fn_lines;
        m.max_fn_name = "big_fn".to_string();
        m.max_nesting = nesting;
        m
    }

    fn baselined(path: &str, fn_lines: usize, nesting: usize) -> Baseline {
        let mut b = Baseline::default();
        b.accepted_structure
            .insert(path.to_string(), Structure { max_fn_lines: fn_lines, max_nesting: nesting });
        b
    }

    /// **The property the whole change exists for.** A file may grow without
    /// limit in LINES; only its structure is gated. Under the old model this
    /// was a `Grew` failure, and it is what made unrelated PRs fail.
    #[test]
    fn a_file_that_only_grew_in_lines_does_not_fail() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 14_516, 40, 3)],
            &baselined("crates/a/src/big.rs", 40, 3),
        );
        assert!(v.iter().all(|x| !x.is_failure()), "{v:?}");
    }

    #[test]
    fn a_new_file_with_an_over_long_function_fails() {
        let v = compare(
            &[metric("crates/a/src/new.rs", 100, FUNCTION_CEILING + 1, 1)],
            &Baseline::default(),
        );
        assert!(matches!(v.as_slice(), [Verdict::NewViolation { .. }]), "{v:?}");
        assert!(v[0].is_failure());
        assert!(v[0].describe().contains("fn"), "{}", v[0].describe());
    }

    #[test]
    fn a_new_file_that_nests_too_deep_fails() {
        let v = compare(
            &[metric("crates/a/src/new.rs", 100, 10, NESTING_CEILING + 1)],
            &Baseline::default(),
        );
        assert!(matches!(v.as_slice(), [Verdict::NewViolation { .. }]), "{v:?}");
    }

    /// Exact, not proportional — a single extra line on the longest function
    /// fails. The old ±10% band is precisely what let debt drift sub-threshold
    /// until an unrelated PR was the straw.
    #[test]
    fn one_more_line_on_an_accepted_function_fails_with_no_tolerance_band() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 900, 121, 3)],
            &baselined("crates/a/src/big.rs", 120, 3),
        );
        assert!(matches!(v.as_slice(), [Verdict::Grew { .. }]), "{v:?}");
        assert!(v[0].describe().contains("120 → 121"), "{}", v[0].describe());
    }

    #[test]
    fn an_accepted_file_holding_steady_passes() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 3000, 120, 6)],
            &baselined("crates/a/src/big.rs", 120, 6),
        );
        assert!(v.iter().all(|x| !x.is_failure()), "{v:?}");
    }

    /// Improving must never fail the build — that would teach the wrong lesson.
    #[test]
    fn shrinking_below_every_ceiling_is_reported_as_resolved_not_failed() {
        let v = compare(
            &[metric("crates/a/src/big.rs", 3000, FUNCTION_CEILING - 1, NESTING_CEILING - 1)],
            &baselined("crates/a/src/big.rs", 300, 9),
        );
        assert!(matches!(v.as_slice(), [Verdict::Resolved { .. }]), "{v:?}");
        assert!(!v[0].is_failure());
    }

    /// A file `syn` could not parse reports ZEROES for these fields. Counting
    /// that as an improvement would silently launder real debt out of the
    /// baseline, so it is skipped entirely.
    #[test]
    fn an_unparseable_file_is_skipped_rather_than_read_as_improved() {
        let mut m = metric("crates/a/src/broken.rs", 3000, 0, 0);
        m.parse_failed = true;
        let v = compare(&[m], &baselined("crates/a/src/broken.rs", 300, 9));
        assert!(v.is_empty(), "an unparseable file must produce no verdict: {v:?}");
    }

    /// `bless` records structure for gating AND line counts for the report.
    #[test]
    fn bless_records_both_the_gated_structure_and_the_reported_size() {
        let b = bless(&[metric("crates/a/src/big.rs", 9000, 300, 9)], "note");
        assert_eq!(b.accepted.get("crates/a/src/big.rs"), Some(&9000));
        assert_eq!(
            b.accepted_structure.get("crates/a/src/big.rs"),
            Some(&Structure { max_fn_lines: 300, max_nesting: 9 })
        );
    }

    /// A baselined file may grow a metric that is still WITHIN its ceiling.
    ///
    /// A file enters the baseline because ONE metric breached. Freezing every
    /// other metric on it at whatever value it happened to hold is not a ratchet,
    /// it is a trap: `remote_hub.rs` was failed for nesting 2 → 3 against a
    /// ceiling of 4, on a change that added a perfectly ordinary `if let` inside
    /// a match arm. A gate that fires on things which are not problems teaches
    /// people to reach for `bless`, which is how the previous mechanism
    /// grandfathered the debt it existed to prevent.
    #[test]
    fn growth_that_stays_under_the_ceiling_does_not_fail() {
        let path = "crates/a/src/mixed.rs";
        // Longest fn is over the ceiling (that is why the file is baselined) and
        // did NOT grow; nesting grew 2 → 3 but the ceiling is 4.
        let v = compare(
            &[metric(path, 900, 120, 3)],
            &baselined(path, 120, 2),
        );
        assert_eq!(
            v.iter().filter(|x| x.is_failure()).count(),
            0,
            "growth below the ceiling is not debt: {v:?}"
        );
    }

    /// ...but crossing the ceiling still fails, so the fix above did not
    /// disable the gate.
    #[test]
    fn growth_that_crosses_the_ceiling_still_fails() {
        let path = "crates/a/src/mixed.rs";
        let v = compare(
            &[metric(path, 900, 120, crate::scan::NESTING_CEILING + 1)],
            &baselined(path, 120, 2),
        );
        assert!(
            v.iter().any(|x| matches!(x, Verdict::Grew { .. })),
            "crossing the nesting ceiling must still be reported: {v:?}"
        );
    }

    /// An already-breaching metric getting worse is the case the baseline exists
    /// for, and must still fail.
    #[test]
    fn an_already_breaching_metric_getting_worse_still_fails() {
        let path = "crates/a/src/big.rs";
        let v = compare(&[metric(path, 900, 514, 2)], &baselined(path, 513, 2));
        assert!(
            v.iter().any(|x| matches!(x, Verdict::Grew { .. })),
            "a 513 → 514 line function is exactly what the baseline guards: {v:?}"
        );
    }
}
