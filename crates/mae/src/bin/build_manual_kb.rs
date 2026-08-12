//! Build-time tool: generate a pre-built CozoDB manual KB file.
//!
//! Usage:
//!   cargo run --bin build-manual-kb -- [output_path]
//!
//! Defaults to `assets/mae-manual.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.
//!
//! Shared checksum/sidecar + fresh-store-open plumbing lives in
//! `mae_kb::kb_build` (ADR-076 D3) — this binary's own unique behavior is
//! the code-gen seed (commands/keymaps/hooks) plus its org-content half.

use mae_kb::kb_build;
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-manual.cozo".into());
    let output_path = PathBuf::from(&output);

    eprintln!("Building manual KB...");

    // Build the in-memory KB from seed content.
    let commands = mae_core::commands::CommandRegistry::with_builtins();
    let keymaps = mae_core::Editor::default_keymaps();
    let hooks = mae_core::hooks::HookRegistry::new();
    let kb = mae_core::kb_seed::seed_kb(&commands, &keymaps, &hooks);

    let node_count = kb.len();
    eprintln!("  Seed KB (code-generated): {node_count} nodes");

    // Open a fresh CozoDB store (removes any existing DB, seeds the
    // relationship-type system) and persist all code-generated nodes.
    let store = kb_build::open_fresh_store(&output_path, kb_build::RELEASE_ASSET_ENGINE)
        .expect("failed to open CozoDB for manual KB output");

    let persisted = store
        .persist_nodes(&kb)
        .expect("failed to persist nodes to CozoDB");
    eprintln!("  Persisted code-generated: {persisted} nodes");
    eprintln!("  Type system seeded");

    // Parse org files from assets/manual/ and ingest into the store. Unlike the
    // guidance corpora this is additive over the code-generated nodes above —
    // the org content deliberately upserts richer prose over its terser
    // `seed_kb()` counterpart — so the manual drives the pieces rather than
    // calling `build_org_kb`, which opens a *fresh* store.
    let manual_dir = PathBuf::from("assets/manual");
    if manual_dir.is_dir() {
        kb_build::ingest_org_dir(&store, &manual_dir, mae_kb::NodeSource::Seed)
            .expect("failed to ingest assets/manual");
    } else {
        eprintln!("  Warning: assets/manual/ not found, skipping org content");
    }

    // Seed typed relationships from code (cmd→category, etc.).
    match store.seed_typed_relationships() {
        Ok(n) => eprintln!("  Code-generated relationships: {n}"),
        Err(e) => eprintln!("  Warning: typed relationships: {e}"),
    }

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
