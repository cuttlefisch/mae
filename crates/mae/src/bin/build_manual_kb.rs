//! Build-time tool: generate a pre-built CozoDB manual KB file.
//!
//! Usage:
//!   cargo run --bin build-manual-kb -- [output_path]
//!
//! Defaults to `assets/mae-manual.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.

use mae_kb::KbStore;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/mae-manual.cozo".into());
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

    eprintln!("Building manual KB...");

    // Build the in-memory KB from seed content.
    let commands = mae_core::commands::CommandRegistry::with_builtins();
    let keymaps = mae_core::Editor::default_keymaps();
    let hooks = mae_core::hooks::HookRegistry::new();
    let kb = mae_core::kb_seed::seed_kb(&commands, &keymaps, &hooks);

    let node_count = kb.len();
    eprintln!("  Seed KB (code-generated): {node_count} nodes");

    // Open a fresh CozoDB store and persist all nodes.
    let store =
        mae_kb::CozoKbStore::open(&output_path).expect("failed to open CozoDB for manual KB");

    let persisted = store
        .persist_nodes(&kb)
        .expect("failed to persist nodes to CozoDB");
    eprintln!("  Persisted code-generated: {persisted} nodes");

    // Seed type system first (needed for known_rel_types during org parsing).
    store
        .seed_type_system()
        .expect("failed to seed type system");
    eprintln!("  Type system seeded");

    // Parse org files from assets/manual/ and ingest into the store.
    let manual_dir = PathBuf::from("assets/manual");
    if manual_dir.is_dir() {
        let known_types = store.known_rel_types().unwrap_or_default();
        let mut all_nodes = Vec::new();
        let mut all_typed_links = Vec::new();
        let mut all_transclusions = Vec::new();

        // Read and parse each .org file.
        let mut org_files: Vec<_> = std::fs::read_dir(&manual_dir)
            .expect("failed to read assets/manual/")
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
            let result = mae_kb::org::parse_org_multi_result(&content, Some(&known_types));
            all_nodes.extend(result.nodes);
            all_typed_links.extend(result.typed_links);
            all_transclusions.extend(result.transclusions);
        }

        let org_node_count = all_nodes.len();
        let typed_link_count = all_typed_links.len();
        let transclusion_count = all_transclusions.len();

        // Save org-parsed nodes to the store.
        let node_refs: Vec<&mae_kb::Node> = all_nodes.iter().collect();
        if let Err(e) = store.save_all(&node_refs) {
            eprintln!("  Warning: failed to save org nodes: {}", e);
        }
        eprintln!(
            "  Org files: {} files, {org_node_count} nodes parsed",
            org_files.len()
        );

        // Wire typed links.
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

        // Wire transclusions.
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

    // Compute SHA-256 checksum over the DB directory contents.
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
/// For sled (directory-based), we hash all files sorted by name.
/// For single-file backends, we hash the file directly.
fn compute_db_checksum(path: &PathBuf) -> String {
    let mut hasher = Sha256::new();

    if path.is_dir() {
        // Sled backend: hash all files in sorted order for determinism.
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

    format!("{:x}", hasher.finalize())
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
