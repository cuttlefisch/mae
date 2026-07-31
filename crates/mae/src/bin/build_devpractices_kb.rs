//! Build-time tool: generate a pre-built CozoDB DevPractices KB file (issue #514, ADR-076).
//!
//! Companion to `build-practices-kb` — same pipeline shape, forked content: generic
//! developer-guidance practices (GitHub/GitLab workflows, code quality, AI collaboration,
//! etc.) for anyone using MAE to build software OTHER than MAE itself, as distinct from
//! `build-practices-kb`'s MAE-specific contributor guidance. See `assets/devpractices/*.org`
//! for the source content (forked from `~/Projects/dev-practices-kb`, ADR-076 D2) and
//! `crates/mae/src/devpractices_kb.rs` for how the built file is located and auto-registered
//! at runtime.
//!
//! Shared checksum/sidecar + fresh-store-open + org-ingestion plumbing lives in
//! `mae_kb::kb_build` (ADR-076 D3).
//!
//! Usage:
//!   cargo run --bin build-devpractices-kb -- [output_path]
//!
//! Defaults to `assets/mae-devpractices.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.

use mae_kb::kb_build;
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-devpractices.cozo".into());
    let output_path = PathBuf::from(&output);

    eprintln!("Building devpractices KB...");

    let store = kb_build::open_fresh_store(&output_path);
    eprintln!("  Type system seeded");

    let devpractices_dir = PathBuf::from("assets/devpractices");
    kb_build::ingest_org_dir(&store, &devpractices_dir);
    kb_build::require_index_node(&store, "assets/devpractices");

    store.seed_views().expect("failed to seed views");
    eprintln!("  Views seeded");

    let checksum = kb_build::compute_db_checksum(&output_path);
    kb_build::write_checksum_sidecar(&output_path, &checksum);

    eprintln!("Done.");
    eprintln!("  Output: {}", output_path.display());
    eprintln!("  SHA-256: {checksum}");
    eprintln!(
        "  Checksum: {}",
        output_path.with_extension("cozo.sha256").display()
    );
}
