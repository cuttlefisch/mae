//! Build-time tool: generate a pre-built CozoDB manual KB file.
//!
//! Usage:
//!   cargo run --bin build-manual-kb -- [output_path]
//!
//! Defaults to `assets/mae-manual.cozo` if no output path is given.
//! Also writes a `.sha256` checksum file alongside the output.

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
    eprintln!("  Seed KB: {node_count} nodes");

    // Open a fresh CozoDB store and persist all nodes.
    let store =
        mae_kb::CozoKbStore::open(&output_path).expect("failed to open CozoDB for manual KB");

    let persisted = store
        .persist_nodes(&kb)
        .expect("failed to persist nodes to CozoDB");
    eprintln!("  Persisted: {persisted} nodes");

    // Seed type system, typed relationships, and views.
    store
        .seed_type_system()
        .expect("failed to seed type system");
    eprintln!("  Type system seeded");

    match store.seed_typed_relationships() {
        Ok(n) => eprintln!("  Typed relationships: {n}"),
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
