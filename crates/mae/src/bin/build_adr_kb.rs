//! Build-time tool: generate a pre-built CozoDB ADR-as-KB-node KB file (ADR-059 Phase C).
//!
//! Companion to `build-manual-kb`/`build-practices-kb` — same pipeline shape (fresh store,
//! seed type system, insert nodes, checksum), sourced from `docs/adr/*.md` instead of
//! hand-curated `.org` content, via `mae_kb::adr_parse`/`mae_kb::adr_kb`.
//!
//! **Not** a `kb_build::ingest_org_dir` user (ADR-076 D3): the ADR corpus is `.md`, not
//! `.org`, requires cross-reference/cycle validation (`validate_corpus`) that has no
//! equivalent in the org-ingestion loop, and derives node bodies from a raw per-file scan
//! rather than `org::parse_org_multi_result`. Forcing this into `ingest_org_dir`'s shape
//! would mean bad-fit special-casing inside a supposedly-generic function — this binary
//! shares only the generic fresh-store-open + checksum/sidecar plumbing with its siblings
//! and keeps its own bespoke corpus discovery/validation/node-generation.
//!
//! **Always writes to a fresh, standalone output file** (removed and recreated on every
//! run, same as the sibling build tools) — this build-time tool never attaches to, or
//! writes into, an already-registered/live KB instance, so it structurally cannot race a
//! live collaborative session over the same store (the failure class named in ADR-059's
//! Context section, from real Obsidian generator/sync-race reports). Every node write goes
//! through `KbStore::insert_node` — the same write path any other programmatic KB write
//! uses, never a raw file/DB write — so `update_links_for_node` (ADR-030) runs exactly as
//! it would for any other node, and the reciprocal-link property `mae_kb::adr_kb`'s own
//! tests already verify holds structurally, not by any special-casing here.
//!
//! Usage:
//!   cargo run --bin build-adr-kb -- [output_path]
//!
//! Defaults to `assets/mae-adr.cozo` if no output path is given. Also writes a `.sha256`
//! checksum file alongside the output, matching the sibling tools' convention.

use mae_kb::adr_kb::generate_corpus_nodes;
use mae_kb::adr_parse::{body_after_header, discover_adr_corpus, validate_corpus};
use mae_kb::kb_build;
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-adr.cozo".into());
    let output_path = PathBuf::from(&output);

    eprintln!("Building ADR KB...");

    let adr_dir = PathBuf::from("docs/adr");
    if !adr_dir.is_dir() {
        panic!(
            "docs/adr/ not found -- expected to run from the workspace root with the ADR \
             files checked in"
        );
    }

    let corpus = discover_adr_corpus(&adr_dir).unwrap_or_else(|e| {
        panic!("failed to parse ADR corpus: {e}");
    });
    eprintln!("  Parsed {} ADR files", corpus.len());
    validate_corpus(&corpus).unwrap_or_else(|e| {
        panic!(
            "ADR corpus failed validation (dangling reference or Extends cycle): {e}\n\
             Fix the offending ADR's header before rebuilding the ADR KB."
        );
    });
    eprintln!("  Corpus validated (no dangling references, no Extends cycles)");

    // Derive each ADR's body prose by scanning the directory once (number -> raw file
    // content) rather than guessing a filename from the number+slug — the real filenames
    // don't always match the slug this parser derives from the title (e.g. abbreviations,
    // punctuation choices).
    let mut bodies_by_number: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    for entry in std::fs::read_dir(&adr_dir).expect("failed to read docs/adr/") {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(first_line) = content.lines().next() else {
            continue;
        };
        let Some(rest) = first_line.strip_prefix("# ADR-") else {
            continue;
        };
        let Some(colon_idx) = rest.find(':') else {
            continue;
        };
        let Ok(number) = rest[..colon_idx].trim().parse::<u32>() else {
            continue;
        };
        bodies_by_number.insert(number, body_after_header(&content).to_string());
    }
    let bodies: Vec<(u32, String)> = corpus
        .iter()
        .map(|m| {
            (
                m.number,
                bodies_by_number.get(&m.number).cloned().unwrap_or_default(),
            )
        })
        .collect();

    let nodes = generate_corpus_nodes(&corpus, &bodies);
    eprintln!("  Generated {} nodes", nodes.len());

    let store = kb_build::open_fresh_store(&output_path, kb_build::RELEASE_ASSET_ENGINE)
        .expect("failed to open CozoDB for ADR KB output");
    eprintln!("  Type system seeded");

    // kb_build::insert_nodes (not a raw file/DB write, and not a local insert
    // loop) — it goes through KbStore::insert_node, which is the load-bearing
    // choice that keeps this generator CRDT-write-path-safe, AND it stamps
    // NodeSource::Seed. That stamp is what makes these nodes read-only: this
    // generator previously had its own insert loop and left `source == None`,
    // so an in-editor or MCP `kb_update` on an installed ADR node succeeded
    // and was then silently destroyed by the next `make adr-kb`.
    let inserted = kb_build::insert_nodes(&store, &nodes, mae_kb::NodeSource::Seed);
    eprintln!("  Inserted {inserted}/{} ADR nodes", nodes.len());

    if inserted == 0 {
        panic!("no ADR nodes inserted -- refusing to ship an empty ADR KB");
    }

    store.seed_views().expect("failed to seed views");
    eprintln!("  Views seeded");

    finish(store, &output_path);
    eprintln!(
        "  Checksum: {}",
        output_path.with_extension("cozo.sha256").display()
    );
}

/// Close the store, checksum the finished artifact, and report.
///
/// Split out for length, but the ORDER is the point: `compute_db_checksum` must
/// run after the handle is dropped — see its doc comment.
fn finish(store: mae_kb::CozoKbStore, output_path: &std::path::Path) {
    drop(store);
    let checksum = kb_build::compute_db_checksum(output_path);
    kb_build::write_checksum_sidecar(output_path, &checksum);
    eprintln!("Done.");
    eprintln!("  Output: {}", output_path.display());
    eprintln!("  SHA-256: {checksum}");
}
