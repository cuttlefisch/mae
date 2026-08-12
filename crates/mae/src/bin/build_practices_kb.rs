//! Build-time tool: generate a pre-built CozoDB practices KB file.
//!
//! Companion to `build-manual-kb` — same pipeline, trimmed to just the
//! org-ingestion step (no code-generated command/keymap/hook nodes, since
//! this KB is curated practices content, not a mirror of the live command
//! registry). See `assets/practices/*.org` for the source content and
//! `crates/mae/src/practices_kb.rs` for how the built file is located and
//! auto-registered at runtime (issue #370).
//!
//! Shared checksum/sidecar + fresh-store-open + org-ingestion plumbing
//! lives in `mae_kb::kb_build` (ADR-076 D3).
//!
//! Usage:
//!   cargo run --bin build-practices-kb -- [output_path]
//!
//! Defaults to `assets/mae-practices.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.

use mae_kb::kb_build;
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-practices.cozo".into());
    let output_path = PathBuf::from(&output);

    eprintln!("Building practices KB...");

    let practices_dir = PathBuf::from("assets/practices");
    kb_build::build_org_kb(
        &practices_dir,
        &output_path,
        &kb_build::OrgKbBuildOptions::default(),
    )
    .expect("failed to build practices KB");

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
