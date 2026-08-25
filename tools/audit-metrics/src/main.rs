//! MAE Audit Metrics
//!
//! Computes the mechanical half of `/mae-audit` — file sizes, function lengths,
//! match-arm counts, struct field counts, nesting depth, test density, and the
//! `@ai-caution`/`@stability` marker cross-reference — and gates it in CI.
//!
//! Why this exists: those numbers used to live as prose in
//! `.claude/commands/mae-audit.md` and `ROADMAP.md`, hand-maintained. By
//! 2026-08 fourteen of fifteen tracked file sizes were wrong, one file had
//! grown +96% while its documented number sat unchanged, and the untracked
//! backlog was ~2x what the docs claimed. Prose cannot hold a moving number.
//!
//! Usage:
//!   cd tools/audit-metrics && cargo run --release -- --workspace-root ../..
//!   cd tools/audit-metrics && cargo run --release -- --workspace-root ../.. --check
//!   cd tools/audit-metrics && cargo run --release -- --workspace-root ../.. --bless

mod markers;
mod report;
mod scan;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const METRICS_REL: &str = "docs/AUDIT_METRICS.json";
const BASELINE_REL: &str = "docs/AUDIT_BASELINE.json";

#[derive(Serialize)]
struct Output<'a> {
    /// Ceilings echoed into the artifact so a reader of the JSON alone knows
    /// what it was measured against.
    ceilings: Ceilings,
    files: &'a [scan::FileMetrics],
    markers: &'a markers::MarkerReport,
}

#[derive(Serialize)]
struct Ceilings {
    source_file: usize,
    test_file: usize,
    function: usize,
    match_arms: usize,
    struct_fields: usize,
    nesting: usize,
}

enum Mode {
    Generate,
    Check,
    Bless,
}

fn main() -> ExitCode {
    let mut root = PathBuf::from("../..");
    let mut mode = Mode::Generate;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace-root" => {
                i += 1;
                match args.get(i) {
                    Some(p) => root = PathBuf::from(p),
                    None => {
                        eprintln!("--workspace-root requires a path");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--check" => mode = Mode::Check,
            "--bless" => mode = Mode::Bless,
            other => {
                eprintln!("Unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let root = match root.canonicalize() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Cannot resolve workspace root: {e}");
            return ExitCode::FAILURE;
        }
    };

    let files = scan::collect_files(&root);
    let metrics: Vec<scan::FileMetrics> = files
        .iter()
        .map(|(path, src)| scan::measure(path, src))
        .collect();

    let mut marker_report = markers::scan(&files);
    let roadmap = read_or_empty(&root, "ROADMAP.md");
    let audit_doc = read_or_empty(&root, ".claude/commands/mae-audit.md");
    let baselined: Vec<String> = load_baseline(&root).accepted.keys().cloned().collect();
    markers::cross_reference(&mut marker_report, &roadmap, &audit_doc, &baselined);

    let json = match render_json(&metrics, &marker_report) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Cannot serialise metrics: {e}");
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Generate => generate(&root, &json, &metrics, &marker_report),
        Mode::Bless => do_bless(&root, &json, &metrics),
        Mode::Check => do_check(&root, &metrics, &marker_report),
    }
}

fn render_json(
    metrics: &[scan::FileMetrics],
    marker_report: &markers::MarkerReport,
) -> serde_json::Result<String> {
    let out = Output {
        ceilings: Ceilings {
            source_file: scan::SOURCE_FILE_CEILING,
            test_file: scan::TEST_FILE_CEILING,
            function: scan::FUNCTION_CEILING,
            match_arms: scan::MATCH_ARM_CEILING,
            struct_fields: scan::STRUCT_FIELD_CEILING,
            nesting: scan::NESTING_CEILING,
        },
        files: metrics,
        markers: marker_report,
    };
    let mut s = serde_json::to_string_pretty(&out)?;
    s.push('\n');
    Ok(s)
}

fn read_or_empty(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_default()
}

fn load_baseline(root: &Path) -> report::Baseline {
    std::fs::read_to_string(root.join(BASELINE_REL))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn generate(
    root: &Path,
    json: &str,
    metrics: &[scan::FileMetrics],
    marker_report: &markers::MarkerReport,
) -> ExitCode {
    if let Err(e) = std::fs::write(root.join(METRICS_REL), json) {
        eprintln!("Cannot write {METRICS_REL}: {e}");
        return ExitCode::FAILURE;
    }
    println!("Generated:\n  {}", root.join(METRICS_REL).display());
    println!("{}", report::summarise(metrics));
    print_marker_summary(marker_report);
    let worst = report::worst_offenders(metrics, 15);
    if !worst.is_empty() {
        println!("\n  most ceiling breaches (file size is REPORTED, not gated — see report.rs):");
        for w in worst {
            println!("    {w}");
        }
    }
    ExitCode::SUCCESS
}

fn do_bless(root: &Path, json: &str, metrics: &[scan::FileMetrics]) -> ExitCode {
    if let Err(e) = std::fs::write(root.join(METRICS_REL), json) {
        eprintln!("Cannot write {METRICS_REL}: {e}");
        return ExitCode::FAILURE;
    }
    let baseline = report::bless(
        metrics,
        "Accepted pre-existing ceiling exceptions, recorded at the size they were \
         accepted. The gate fails on NEW violations and on accepted files that grow \
         past the tolerance; it never fails a file for shrinking. Regenerate with \
         `make audit-metrics-bless` ONLY when deliberately accepting new debt.",
    );
    let mut s = match serde_json::to_string_pretty(&baseline) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Cannot serialise baseline: {e}");
            return ExitCode::FAILURE;
        }
    };
    s.push('\n');
    if let Err(e) = std::fs::write(root.join(BASELINE_REL), &s) {
        eprintln!("Cannot write {BASELINE_REL}: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "Blessed {} accepted exception(s) -> {}",
        baseline.accepted.len(),
        root.join(BASELINE_REL).display()
    );
    println!("{}", report::summarise(metrics));
    ExitCode::SUCCESS
}

/// Check mode deliberately does NOT compare against a committed metrics file.
/// Line counts change on essentially every commit, so a committed
/// `AUDIT_METRICS.json` would produce a large, meaningless diff on every PR
/// (the failure mode `docs/CODE_MAP.json` already has, tolerable there only
/// because public-item lists change rarely). Metrics are recomputed in memory;
/// the only committed artifact is the baseline, which changes when debt is
/// deliberately accepted or paid down.
fn do_check(
    root: &Path,
    metrics: &[scan::FileMetrics],
    marker_report: &markers::MarkerReport,
) -> ExitCode {
    let verdicts = report::compare(metrics, &load_baseline(root));
    let failures: Vec<&report::Verdict> = verdicts.iter().filter(|v| v.is_failure()).collect();
    let resolved: Vec<&report::Verdict> = verdicts.iter().filter(|v| !v.is_failure()).collect();

    if !resolved.is_empty() {
        println!("{} baseline entr(ies) now under ceiling:", resolved.len());
        for v in &resolved {
            println!("  {}", v.describe());
        }
        println!("  (run `make audit-metrics-bless` to drop them)");
    }

    if failures.is_empty() {
        println!("Audit metrics up to date; no new or growing ceiling violations.");
        print_marker_summary(marker_report);
        return ExitCode::SUCCESS;
    }

    eprintln!("\n{} ceiling violation(s):", failures.len());
    for v in failures {
        eprintln!("  {}", v.describe());
    }
    eprintln!(
        "\nShorten the function, or reduce the nesting -- both are local, per-item \
         changes, not a file split. If the debt is deliberate, run \
         `make audit-metrics-bless`, add an `@ai-caution: [architecture-debt]` marker, \
         and cross-link it from ROADMAP.md per CLAUDE.md's tagging convention.\n\
         \nNote: file SIZE is measured and reported but no longer gated -- it \
         grandfathered the debt it existed to prevent and failed unrelated PRs. \
         See tools/audit-metrics/src/report.rs for the evidence."
    );
    ExitCode::FAILURE
}

fn print_marker_summary(m: &markers::MarkerReport) {
    println!(
        "  markers: {} @ai-caution, {} @stability",
        m.cautions.len(),
        m.stability.len()
    );
    let mut issues = Vec::new();
    if !m.orphaned_debt_markers.is_empty() {
        issues.push(format!(
            "{} orphaned [architecture-debt] marker(s) (in code, in neither tracking doc): {}",
            m.orphaned_debt_markers.len(),
            m.orphaned_debt_markers.join(", ")
        ));
    }
    if !m.untracked_in_code.is_empty() {
        issues.push(format!(
            "{} tracked exception(s) with no in-code marker: {}",
            m.untracked_in_code.len(),
            m.untracked_in_code.join(", ")
        ));
    }
    if !m.uncategorised.is_empty() {
        issues.push(format!(
            "{} @ai-caution without a [category]: {}",
            m.uncategorised.len(),
            m.uncategorised.join(", ")
        ));
    }
    if !m.missing_stability.is_empty() {
        issues.push(format!(
            "{} crate root(s) missing @stability: {}",
            m.missing_stability.len(),
            m.missing_stability.join(", ")
        ));
    }
    for i in issues {
        println!("  cross-ref: {i}");
    }
}
