//! Build-time tool: generate a pre-built CozoDB ADR-as-KB-node KB file (ADR-059 Phase C).
//!
//! Companion to `build-manual-kb`/`build-practices-kb` — same pipeline shape (fresh store,
//! seed type system, insert nodes, checksum), sourced from `docs/adr/*.md` instead of
//! hand-curated `.org` content, via `mae_kb::adr_parse`/`mae_kb::adr_kb`.
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
use mae_kb::KbStore;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-adr.cozo".into());
    let output_path = PathBuf::from(&output);

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create output directory");
    }
    if output_path.exists() {
        if output_path.is_dir() {
            std::fs::remove_dir_all(&output_path).expect("failed to remove existing DB directory");
        } else {
            std::fs::remove_file(&output_path).expect("failed to remove existing DB file");
        }
    }

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

    let store = mae_kb::CozoKbStore::open(&output_path).expect("failed to open CozoDB for ADR KB");
    store
        .seed_type_system()
        .expect("failed to seed type system");
    eprintln!("  Type system seeded");

    let mut inserted = 0;
    for node in &nodes {
        // KbStore::insert_node (not a raw file/DB write) — see module doc comment on why
        // this is the load-bearing choice that keeps this generator CRDT-write-path-safe.
        match store.insert_node(node) {
            Ok(()) => inserted += 1,
            Err(e) => eprintln!("  Warning: failed to insert node {}: {}", node.id, e),
        }
    }
    eprintln!("  Inserted {inserted}/{} ADR nodes", nodes.len());

    if inserted == 0 {
        panic!("no ADR nodes inserted -- refusing to ship an empty ADR KB");
    }

    store.seed_views().expect("failed to seed views");
    eprintln!("  Views seeded");

    let checksum = compute_db_checksum(&output_path);
    let sha_path = output_path.with_extension("cozo.sha256");
    std::fs::write(
        &sha_path,
        format!("{checksum}  {}\n", output_path.display()),
    )
    .expect("failed to write checksum file");

    eprintln!("Done.");
    eprintln!("  Output: {}", output_path.display());
    eprintln!("  SHA-256: {checksum}");
    eprintln!("  Checksum: {}", sha_path.display());
}

/// Compute a SHA-256 checksum for the CozoDB store (sled: hash every file in sorted order;
/// single-file backends: hash the file directly) — identical convention to the sibling
/// build-manual-kb/build-practices-kb tools.
fn compute_db_checksum(path: &PathBuf) -> String {
    let mut hasher = Sha256::new();
    if path.is_dir() {
        let mut files = Vec::new();
        collect_files_recursive(path, &mut files);
        files.sort();
        for file in &files {
            let rel = file.strip_prefix(path).unwrap_or(file);
            hasher.update(rel.to_string_lossy().as_bytes());
            let data = std::fs::read(file).expect("failed to read DB file for checksum");
            hasher.update(&data);
        }
    } else {
        let data = std::fs::read(path).expect("failed to read DB file for checksum");
        hasher.update(&data);
    }
    hex::encode(hasher.finalize())
}

fn collect_files_recursive(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursive(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}
