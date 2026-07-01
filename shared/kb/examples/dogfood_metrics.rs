//! PII-SAFE dogfood metrics (plan Phase 7 / issue #243).
//!
//! Ingest an org directory and print **only quantitative metrics** — counts, sizes, ratios,
//! timings. It NEVER prints node titles, bodies, tags, ids, filenames, or link targets, so its
//! output is safe to paste into an issue / transcript when validating KB ingestion at real
//! RoamNotes scale. The count-only fields come from `ImportHealth` + the scalar fields of
//! `ImportReport`; every `Vec`/`HashMap` field (which carries ids/paths) is reduced to a
//! `.len()` before it is printed.
//!
//! Usage: `cargo run -p mae-kb --example dogfood_metrics -- <org-dir>`
//!
//! Output is `key value` lines (grep-able); a caller can assert `grep`-absence of any known
//! content string to prove no PII leaked.

use std::path::Path;
use std::time::Instant;

/// Total bytes of all files under `dir` (portable; no `du`).
fn dir_size_bytes(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: dogfood_metrics <org-dir>   (prints PII-safe quant metrics only)");
        std::process::exit(2);
    };
    let path = Path::new(&dir);
    if !path.is_dir() {
        eprintln!("not a directory: {dir}");
        std::process::exit(2);
    }

    let org_dir_bytes = dir_size_bytes(path);
    let t0 = Instant::now();
    let (_kb, report, health) = mae_kb::federation::import_org_dir(path);
    let wall_ms = t0.elapsed().as_millis();

    let links_per_node = if health.total_nodes > 0 {
        (health.total_links as f64 / health.total_nodes as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };

    // COUNT-ONLY. Deliberately never printing: report.duplicate_ids / report.errors /
    // report.path_to_ids (ids + paths), health.namespace_counts KEYS (a namespace can be a bare
    // node slug). Only their cardinalities are safe.
    let metrics: [(&str, String); 16] = [
        ("org_dir_bytes", org_dir_bytes.to_string()),
        ("ingest_wall_ms", wall_ms.to_string()),
        ("import_duration_ms", report.duration_ms.to_string()),
        ("nodes_imported", report.nodes_imported.to_string()),
        ("nodes_skipped", report.nodes_skipped.to_string()),
        ("nodes_updated", report.nodes_updated.to_string()),
        ("nodes_unchanged", report.nodes_unchanged.to_string()),
        ("links_created", report.links_created.to_string()),
        ("import_error_count", report.errors.len().to_string()),
        ("duplicate_id_count", report.duplicate_ids.len().to_string()),
        ("total_nodes", health.total_nodes.to_string()),
        ("total_links", health.total_links.to_string()),
        ("links_per_node", links_per_node.to_string()),
        ("orphan_count", health.orphan_count.to_string()),
        ("broken_link_count", health.broken_link_count.to_string()),
        (
            "distinct_namespaces",
            health.namespace_counts.len().to_string(),
        ),
    ];
    for (k, v) in &metrics {
        println!("{k} {v}");
    }
}
