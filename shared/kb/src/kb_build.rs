//! Shared plumbing for MAE's build-time KB-asset generators (ADR-076 D3).
//!
//! `build_manual_kb.rs`, `build_practices_kb.rs`, `build_adr_kb.rs`, and
//! `build_devpractices_kb.rs` (all in `crates/mae/src/bin/`) each produce a
//! pre-built CozoDB knowledge-base asset shipped with releases. All four
//! shared byte-for-byte identical SHA-256 checksum/sidecar-file logic, and
//! the org-ingestion loop (read_dir → sort → parse → insert_node/
//! add_typed_link/add_meta_member) was structurally identical across the
//! `.org`-sourced binaries (practices, devpractices — manual too, for its
//! org-content half). This module extracts exactly that shared plumbing as
//! free functions, matching the free-function style the build binaries
//! already use (no struct/builder wrapper).
//!
//! Deliberately **not** a parameterized "one binary, many modes" tool — the
//! manual KB (code-gen, no index-node requirement) and the ADR KB
//! (cross-reference/cycle validation, `.md` corpus, no raw org-dir
//! ingestion) have genuinely different input shapes. See ADR-076 D3's
//! "Alternatives considered" for the full rationale.
//!
//! These are build-time tools, not runtime code: every function here
//! panics (with a clear, actionable message) on failure rather than
//! returning `Result`, matching the existing `.expect(...)`-heavy style of
//! the binaries this module was extracted from.

use crate::{CozoKbStore, KbStore, NodeSource};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Counts from a single [`ingest_org_dir`] call, for the caller's own
/// `println!`/`eprintln!` summary reporting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OrgIngestStats {
    /// Number of `.org` files read from the directory.
    pub org_files: usize,
    /// Number of nodes parsed (and inserted) across all files.
    pub nodes: usize,
    /// Number of typed links parsed from node bodies.
    pub typed_links_parsed: usize,
    /// Number of typed links successfully stored.
    pub typed_links_stored: usize,
    /// Number of transclusion directives parsed.
    pub transclusions_parsed: usize,
    /// Number of transclusion directives successfully stored.
    pub transclusions_stored: usize,
}

/// Remove any existing DB at `output_path`, ensure its parent directory
/// exists, open a fresh [`CozoKbStore`], and seed the relationship-type
/// system. This is the shared prologue of every build binary's `main()`.
///
/// Panics with a clear message on any failure — build-time tooling, not
/// runtime code (see module doc comment).
pub fn open_fresh_store(output_path: &Path) -> CozoKbStore {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create output directory");
    }

    // Remove existing DB so we start fresh (sled uses a directory).
    if output_path.exists() {
        if output_path.is_dir() {
            std::fs::remove_dir_all(output_path).expect("failed to remove existing DB directory");
        } else {
            std::fs::remove_file(output_path).expect("failed to remove existing DB file");
        }
    }

    let store = CozoKbStore::open(output_path).expect("failed to open CozoDB for KB build output");

    // Seed the relationship-type system (registry for type validation +
    // introspection; ADR-030 link parsing reads rel from each link's `?query`).
    store
        .seed_type_system()
        .expect("failed to seed type system");

    store
}

/// Ingest every `.org` file in `dir` (sorted by path for determinism):
/// parse with `org::parse_org_multi_result`, then `insert_node` each parsed
/// node (tagged [`NodeSource::Seed`]), `add_typed_link` each typed link, and
/// `add_meta_member` each transclusion directive.
///
/// Panics if `dir` does not exist (the caller is almost certainly not
/// running from the workspace root) or if zero nodes were parsed (refusing
/// to silently ship an empty KB). Per-file read errors and per-item
/// store-write errors are logged as warnings and skipped, not fatal —
/// matching the existing binaries' behavior of getting as much content in
/// as possible rather than aborting on one bad node/link.
pub fn ingest_org_dir(store: &CozoKbStore, dir: &Path) -> OrgIngestStats {
    if !dir.is_dir() {
        panic!(
            "{} not found -- expected to run from the workspace root with the seed .org \
             files checked in",
            dir.display()
        );
    }

    let mut all_nodes = Vec::new();
    let mut all_typed_links = Vec::new();
    let mut all_transclusions = Vec::new();

    let mut org_files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
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
        let result = crate::org::parse_org_multi_result(&content);
        all_nodes.extend(
            result
                .nodes
                .into_iter()
                .map(|n| n.with_source(NodeSource::Seed, 1)),
        );
        all_typed_links.extend(result.typed_links);
        all_transclusions.extend(result.transclusions);
    }

    let node_count = all_nodes.len();
    let typed_link_count = all_typed_links.len();
    let transclusion_count = all_transclusions.len();

    // Insert org-parsed nodes into the store (upsert, no delete). We use
    // insert_node rather than replace_all_nodes to avoid the CozoDB sled tombstone
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
        panic!(
            "no nodes parsed from {} -- refusing to ship an empty KB",
            dir.display()
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

    OrgIngestStats {
        org_files: org_files.len(),
        nodes: node_count,
        typed_links_parsed: typed_link_count,
        typed_links_stored: link_count,
        transclusions_parsed: transclusion_count,
        transclusions_stored: trans_count as usize,
    }
}

/// Panic if `store` has no node with the literal id `"index"`.
///
/// `read_guidance_kb_context` (`crates/ai/src/guidance.rs`) looks up
/// exactly that id for whichever KB instance `ai_guidance_kb` names, so a
/// guidance KB shipped without one would silently fail to surface any
/// content at read time instead of failing loudly at build time.
///
/// `source_dir_hint` names the caller's source directory in the panic
/// message (e.g. `"assets/practices"`), so each caller's failure correctly
/// points at its own content rather than a generic/wrong location.
pub fn require_index_node(store: &CozoKbStore, source_dir_hint: &str) {
    if store.get_node("index").ok().flatten().is_none() {
        panic!(
            "{source_dir_hint}/index.org must define node id \"index\" (literal, not \
             namespaced) -- guidance.rs::read_guidance_kb_context() looks up exactly that \
             id for whichever KB instance ai_guidance_kb names"
        );
    }
}

/// Compute a SHA-256 checksum for the CozoDB store.
///
/// For sled (directory-based), hashes all files in sorted order for
/// determinism. For single-file backends, hashes the file directly.
pub fn compute_db_checksum(path: &Path) -> String {
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

/// Write the `<name>.cozo.sha256` sidecar file for `output_path`, in the
/// `"{checksum}  {path}\n"` format every build binary uses.
pub fn write_checksum_sidecar(output_path: &Path, checksum: &str) {
    let sha_path = output_path.with_extension("cozo.sha256");
    std::fs::write(
        &sha_path,
        format!("{checksum}  {}\n", output_path.display()),
    )
    .expect("failed to write checksum file");
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_org(dir: &Path, filename: &str, content: &str) {
        std::fs::write(dir.join(filename), content).expect("failed to write fixture org file");
    }

    // --- compute_db_checksum ---

    #[test]
    fn checksum_is_deterministic_across_repeated_calls() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"world").unwrap();

        let first = compute_db_checksum(tmp.path());
        let second = compute_db_checksum(tmp.path());
        assert_eq!(
            first, second,
            "checksum of an unchanged directory must be stable across calls"
        );
    }

    /// Adversarial half of the determinism test: a real content change MUST
    /// change the checksum. A test that only asserts "it doesn't crash" or
    /// "two computations of the same input match" would pass even if the
    /// hasher silently ignored file contents.
    #[test]
    fn checksum_changes_when_file_content_changes() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let before = compute_db_checksum(tmp.path());

        std::fs::write(tmp.path().join("a.txt"), b"hello, world").unwrap();
        let after = compute_db_checksum(tmp.path());

        assert_ne!(
            before, after,
            "changing a tracked file's bytes must change the checksum"
        );
    }

    #[test]
    fn checksum_of_single_file_matches_direct_read() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("single.cozo");
        std::fs::write(&file, b"some db bytes").unwrap();

        let mut hasher = Sha256::new();
        hasher.update(b"some db bytes");
        let expected = hex::encode(hasher.finalize());

        assert_eq!(compute_db_checksum(&file), expected);
    }

    #[test]
    fn write_checksum_sidecar_writes_expected_format() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("mae-test.cozo");
        write_checksum_sidecar(&output, "deadbeef");

        let sha_path = output.with_extension("cozo.sha256");
        let contents = std::fs::read_to_string(&sha_path).unwrap();
        assert_eq!(contents, format!("deadbeef  {}\n", output.display()));
    }

    // --- ingest_org_dir ---

    const NODE_A: &str =
        ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Node A\n\nBody of A, links to [[test:b][B]].\n";
    const NODE_B: &str =
        ":PROPERTIES:\n:ID: test:b\n:END:\n#+title: Node B\n\nBody of B, no outgoing links.\n";
    const NODE_C: &str = ":PROPERTIES:\n:ID: test:c\n:END:\n#+title: Node C\n\nBody of C, links to [[test:a][A]] and [[test:b][B]].\n";

    #[test]
    fn ingest_org_dir_counts_nodes_and_links_exactly() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);
        write_org(src_dir.path(), "b.org", NODE_B);
        write_org(src_dir.path(), "c.org", NODE_C);

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let stats = ingest_org_dir(&store, src_dir.path());

        assert_eq!(stats.org_files, 3);
        assert_eq!(stats.nodes, 3, "exactly 3 nodes across the 3 fixture files");
        // a -> b, c -> a, c -> b = 3 typed links total, all should store cleanly.
        assert_eq!(stats.typed_links_parsed, 3);
        assert_eq!(stats.typed_links_stored, 3);
        assert_eq!(stats.transclusions_parsed, 0);
        assert_eq!(stats.transclusions_stored, 0);

        // Selective oracle: not just "> 0", confirm the actual node made it
        // into the store with the right id and content survived the round
        // trip (principle #14 — pin the meaningful outcome).
        let node_a = store
            .get_node("test:a")
            .expect("query ok")
            .expect("node test:a must exist after ingest");
        assert_eq!(node_a.title, "Node A");
    }

    #[test]
    fn ingest_org_dir_ignores_non_org_files() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);
        std::fs::write(src_dir.path().join("README.md"), "not an org file").unwrap();
        std::fs::write(src_dir.path().join("notes.txt"), "also not org").unwrap();

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let stats = ingest_org_dir(&store, src_dir.path());
        assert_eq!(stats.org_files, 1);
        assert_eq!(stats.nodes, 1);
    }

    #[test]
    #[should_panic(expected = "not found")]
    fn ingest_org_dir_panics_on_missing_directory() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        ingest_org_dir(
            &store,
            Path::new("/nonexistent/definitely-not-a-real-dir-9f3c"),
        );
    }

    #[test]
    #[should_panic(expected = "refusing to ship an empty KB")]
    fn ingest_org_dir_panics_on_zero_nodes() {
        let src_dir = TempDir::new().unwrap();
        // Directory exists but has no .org files.
        std::fs::write(src_dir.path().join("README.md"), "nothing to ingest").unwrap();

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        ingest_org_dir(&store, src_dir.path());
    }

    // --- require_index_node ---

    #[test]
    #[should_panic(expected = "must define node id \"index\"")]
    fn require_index_node_panics_on_fresh_store_with_no_index() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        require_index_node(&store, "assets/fixture-kb");
    }

    #[test]
    fn require_index_node_passes_when_index_node_present() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let src_dir = TempDir::new().unwrap();
        write_org(
            src_dir.path(),
            "index.org",
            ":PROPERTIES:\n:ID: index\n:END:\n#+title: Index\n\nEntry point.\n",
        );
        ingest_org_dir(&store, src_dir.path());

        // Should not panic.
        require_index_node(&store, "assets/fixture-kb");
    }

    // --- open_fresh_store ---

    #[test]
    fn open_fresh_store_creates_parent_dirs_and_removes_stale_output() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("nested").join("dir").join("out.cozo");

        // First open: parent dirs don't exist yet.
        {
            let _store = open_fresh_store(&output);
        }
        assert!(output.exists(), "store output should exist after opening");

        // Write a marker file inside the sled directory to prove the second
        // open really starts fresh (removes the old directory contents)
        // rather than reusing stale state.
        let marker = output.join("__marker__");
        if output.is_dir() {
            std::fs::write(&marker, b"stale").unwrap();
        }

        {
            let _store = open_fresh_store(&output);
        }
        if output.is_dir() {
            assert!(
                !marker.exists(),
                "open_fresh_store must remove the prior store before reopening"
            );
        }
    }
}
