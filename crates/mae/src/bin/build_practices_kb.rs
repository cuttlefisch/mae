//! Build-time tool: generate a pre-built CozoDB practices KB file.
//!
//! Companion to `build-manual-kb` — same pipeline, trimmed to just the
//! org-ingestion step (no code-generated command/keymap/hook nodes, since
//! this KB is curated practices content, not a mirror of the live command
//! registry). See `assets/practices/*.org` for the source content and
//! `crates/mae/src/practices_kb.rs` for how the built file is located and
//! auto-registered at runtime (issue #370).
//!
//! Usage:
//!   cargo run --bin build-practices-kb -- [output_path]
//!
//! Defaults to `assets/mae-practices.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.

use mae_kb::KbStore;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-practices.cozo".into());
    let output_path = PathBuf::from(&output);

    // Ensure parent directory exists.
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create output directory");
    }

    // Remove existing DB so we start fresh (sled uses a directory).
    if output_path.exists() {
        if output_path.is_dir() {
            std::fs::remove_dir_all(&output_path).expect("failed to remove existing DB directory");
        } else {
            std::fs::remove_file(&output_path).expect("failed to remove existing DB file");
        }
    }

    eprintln!("Building practices KB...");

    let store =
        mae_kb::CozoKbStore::open(&output_path).expect("failed to open CozoDB for practices KB");

    // Seed the relationship-type system (registry for type validation +
    // introspection; ADR-030 link parsing reads rel from each link's `?query`).
    store
        .seed_type_system()
        .expect("failed to seed type system");
    eprintln!("  Type system seeded");

    let practices_dir = PathBuf::from("assets/practices");
    if !practices_dir.is_dir() {
        panic!(
            "assets/practices/ not found -- expected to run from the workspace root with the \
             seed .org files checked in"
        );
    }

    let mut all_nodes = Vec::new();
    let mut all_typed_links = Vec::new();
    let mut all_transclusions = Vec::new();

    let mut org_files: Vec<_> = std::fs::read_dir(&practices_dir)
        .expect("failed to read assets/practices/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "org"))
        .collect();
    org_files.sort();

    for path in &org_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  Warning: failed to read {}: {}", path.display(), e);
                continue;
            }
        };
        let result = mae_kb::org::parse_org_multi_result(&content);
        all_nodes.extend(
            result
                .nodes
                .into_iter()
                .map(|n| n.with_source(mae_kb::NodeSource::Seed, 1)),
        );
        all_typed_links.extend(result.typed_links);
        all_transclusions.extend(result.transclusions);
    }

    let node_count = all_nodes.len();
    let typed_link_count = all_typed_links.len();
    let transclusion_count = all_transclusions.len();

    // Insert org-parsed nodes into the store (upsert, no delete). We use
    // insert_node rather than save_all to avoid the CozoDB sled tombstone
    // issue: :rm leaves partial tuples that break load_all().
    for node in &all_nodes {
        if let Err(e) = store.insert_node(node) {
            eprintln!("  Warning: failed to insert node {}: {}", node.id, e);
        }
    }
    eprintln!(
        "  Org files: {} files, {node_count} nodes parsed",
        org_files.len()
    );

    if node_count == 0 {
        panic!("no nodes parsed from assets/practices/ -- refusing to ship an empty practices KB");
    }
    if store.get_node("index").ok().flatten().is_none() {
        panic!(
            "assets/practices/index.org must define node id \"index\" (literal, not \
             namespaced) -- guidance.rs::read_guidance_kb_context() looks up exactly that \
             id for whichever KB instance ai_guidance_kb names"
        );
    }

    let mut link_count = 0;
    for (src, link) in &all_typed_links {
        if let Err(e) = store.add_typed_link(src, &link.target, &link.rel_type, 1.0) {
            eprintln!(
                "  Warning: typed link {}→{} ({}): {}",
                src, link.target, link.rel_type, e
            );
        } else {
            link_count += 1;
        }
    }
    eprintln!("  Typed links: {typed_link_count} parsed, {link_count} stored");

    let mut trans_count = 0;
    for (meta_id, member_id, role) in &all_transclusions {
        if let Err(e) = store.add_meta_member(meta_id, member_id, trans_count, role) {
            eprintln!("  Warning: transclusion {meta_id}←{member_id}: {e}");
        } else {
            trans_count += 1;
        }
    }
    if transclusion_count > 0 {
        eprintln!("  Transclusions: {transclusion_count} parsed, {trans_count} stored");
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

/// Compute a SHA-256 checksum for the CozoDB store.
///
/// For sled (directory-based), we hash all files in sorted order for
/// determinism. For single-file backends, we hash the file directly.
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
