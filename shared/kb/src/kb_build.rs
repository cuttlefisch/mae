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
//! Still **not** a parameterized "one binary, many modes" tool — the manual KB
//! (code-gen, no index-node requirement) and the ADR KB (cross-reference/cycle
//! validation, `.md` corpus, no raw org-dir ingestion) have genuinely
//! different input shapes, exactly as ADR-076 D3's "Alternatives considered"
//! argued. [`build_org_kb`] composes the shared org-corpus pipeline that the
//! practices and devpractices binaries had duplicated; the other two still
//! drive the pieces themselves.
//!
//! ## These are no longer build-time-only
//!
//! This module used to panic on every failure, justified by "build-time tools,
//! not runtime code". That justification does not survive two new callers:
//!
//! - **Tests.** `guidance.rs` and `bootstrap.rs` build a real KB from the
//!   tracked `assets/*.org` corpora instead of depending on a `make` target
//!   having been run. A `panic!` there is merely a bad failure message; a
//!   `Result` names which corpus and why.
//! - **Runtime provisioning.** Building a system KB on the user's machine is
//!   the direction this is headed, and there a malformed corpus becoming a
//!   startup panic would be a genuinely worse failure mode than a warning and
//!   a degraded-but-running editor.
//!
//! So the fallible functions return [`KbBuildError`]. The build binaries keep
//! `.expect(...)` at `main()`, which preserves their loud, actionable
//! build-time failure exactly as before.
//!
//! ## One write boundary
//!
//! [`insert_nodes`] is the single place a build-time generator writes nodes
//! into a store, and it stamps [`NodeSource`] itself. That is deliberate:
//! provenance was previously stamped at each generator's own discretion
//! (`kb_seed::stamp_source`, `ingest_org_dir`'s `with_source`) and the ADR-KB
//! generator simply forgot, leaving its nodes with `source == None`. Since the
//! `NodeSource::Seed` guard in `kb_update_node_with` only refuses
//! `Some(NodeSource::Seed)`, an edit to an installed ADR node succeeded and was
//! then silently destroyed by the next build. Stamping at the write boundary
//! means a future fifth generator cannot reintroduce that class of bug by
//! omission.

use crate::{CozoKbStore, KbStore, Node, NodeSource};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Storage engine used for KB assets built by the shipped `build-*-kb`
/// binaries.
///
/// Kept at sled so this refactor does not silently change the format of a
/// release artifact; the delivery cutover is a separate, deliberate change.
/// Callers that do not ship their output — tests, and eventually runtime
/// provisioning — should pass `"sqlite"`, which is a single file, needs no
/// lock-file stripping, and is what `kb_storage_engine` defaults to anyway.
pub const RELEASE_ASSET_ENGINE: &str = "sled";

/// A build step failed. Carries enough context to name the corpus and the
/// reason without the caller reconstructing either.
#[derive(Debug)]
pub enum KbBuildError {
    /// The source corpus directory does not exist.
    MissingSourceDir(PathBuf),
    /// The corpus parsed to zero nodes — refusing to produce an empty KB.
    EmptyCorpus(PathBuf),
    /// A guidance corpus produced no node with the literal id `index`.
    MissingIndexNode(PathBuf),
    /// Filesystem error preparing or removing the output location.
    Io(PathBuf, std::io::Error),
    /// The underlying store rejected an open or a write.
    Store(String),
}

impl std::fmt::Display for KbBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSourceDir(p) => write!(
                f,
                "{} not found -- expected to run from the workspace root with the seed \
                 .org files checked in",
                p.display()
            ),
            Self::EmptyCorpus(p) => write!(
                f,
                "no nodes parsed from {} -- refusing to ship an empty KB",
                p.display()
            ),
            Self::MissingIndexNode(p) => write!(
                f,
                "{}/index.org must define node id \"index\" (literal, not namespaced) -- \
                 guidance.rs::read_guidance_kb_context() looks up exactly that id for \
                 whichever KB instance ai_guidance_kb names",
                p.display()
            ),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Store(e) => write!(f, "KB store error: {e}"),
        }
    }
}

impl std::error::Error for KbBuildError {}

/// How to build an org-corpus KB. See [`build_org_kb`].
#[derive(Debug, Clone)]
pub struct OrgKbBuildOptions {
    /// Storage engine, e.g. [`RELEASE_ASSET_ENGINE`] or `"sqlite"`.
    pub engine: &'static str,
    /// Provenance stamped on every node written. [`NodeSource::Seed`] for
    /// anything MAE ships — it is what makes the content read-only.
    pub source: NodeSource,
    /// Require a literal `index` node, as guidance KBs must have one.
    pub require_index: bool,
    /// Seed the stored Datalog views (kanban/backlog/sprint/agenda).
    pub seed_views: bool,
}

impl Default for OrgKbBuildOptions {
    /// The shape every shipped guidance corpus uses.
    fn default() -> Self {
        Self {
            engine: RELEASE_ASSET_ENGINE,
            source: NodeSource::Seed,
            require_index: true,
            seed_views: true,
        }
    }
}

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
/// exists, open a fresh [`CozoKbStore`] on `engine`, and seed the
/// relationship-type system. This is the shared prologue of every build
/// binary's `main()`.
///
/// `engine` is a parameter rather than the hardcoded sled that
/// `CozoKbStore::open` implies: the daemon is compiled sqlite-only and cannot
/// open a sled store at all (the failure is a runtime `bail!`, not a compile
/// error), and `kb_storage_engine` defaults to sqlite, so a sled asset is
/// migrated in place the first time it is registered. Pass
/// [`RELEASE_ASSET_ENGINE`] to keep producing today's release format.
pub fn open_fresh_store(output_path: &Path, engine: &str) -> Result<CozoKbStore, KbBuildError> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| KbBuildError::Io(parent.to_path_buf(), e))?;
    }

    // Remove existing DB so we start fresh (sled uses a directory, sqlite a file).
    if output_path.exists() {
        let removed = if output_path.is_dir() {
            std::fs::remove_dir_all(output_path)
        } else {
            std::fs::remove_file(output_path)
        };
        removed.map_err(|e| KbBuildError::Io(output_path.to_path_buf(), e))?;
    }

    let store = CozoKbStore::open_with_engine(output_path, engine)
        .map_err(|e| KbBuildError::Store(e.to_string()))?;

    // Seed the relationship-type system (registry for type validation +
    // introspection; ADR-030 link parsing reads rel from each link's `?query`).
    store
        .seed_type_system()
        .map_err(|e| KbBuildError::Store(e.to_string()))?;

    Ok(store)
}

/// Write `nodes` into `store`, stamping every one with `source`.
///
/// **The single node-write boundary for build-time generators.** Stamping here
/// rather than at each generator is what makes provenance un-forgettable — see
/// this module's doc comment for the ADR-KB bug that omission caused.
///
/// Per-node write errors are warned and skipped rather than fatal, matching
/// the generators' existing "get as much content in as possible" behavior. The
/// returned count is how many actually landed, so the caller can still refuse
/// to ship an empty KB.
pub fn insert_nodes(store: &CozoKbStore, nodes: &[Node], source: NodeSource) -> usize {
    let mut inserted = 0;
    for node in nodes {
        let stamped = node.clone().with_source(source, 1);
        match store.insert_node(&stamped) {
            Ok(()) => inserted += 1,
            Err(e) => eprintln!("  Warning: failed to insert node {}: {}", node.id, e),
        }
    }
    inserted
}

/// Build a complete KB from a directory of `.org` files: open a fresh store,
/// ingest the corpus, optionally require an `index` node, optionally seed
/// views.
///
/// This is the pipeline the practices and devpractices binaries had duplicated
/// line for line, and it is what tests should call to exercise *the real
/// shipped corpus* — `assets/practices`/`assets/devpractices` are the tracked
/// source of truth, so building from them in-process is strictly more faithful
/// than opening a pre-built artifact that may not have been regenerated.
pub fn build_org_kb(
    src_dir: &Path,
    output_path: &Path,
    opts: &OrgKbBuildOptions,
) -> Result<OrgIngestStats, KbBuildError> {
    let store = open_fresh_store(output_path, opts.engine)?;
    let stats = ingest_org_dir(&store, src_dir, opts.source)?;

    if opts.require_index {
        require_index_node(&store, src_dir)?;
    }
    if opts.seed_views {
        store
            .seed_views()
            .map_err(|e| KbBuildError::Store(e.to_string()))?;
    }
    Ok(stats)
}

/// Ingest every `.org` file in `dir` (sorted by path for determinism):
/// parse with `org::parse_org_multi_result`, then `insert_node` each parsed
/// node (tagged [`NodeSource::Seed`]), `add_typed_link` each typed link, and
/// `add_meta_member` each transclusion directive.
///
/// Errors if `dir` does not exist (the caller is almost certainly not
/// running from the workspace root) or if zero nodes were parsed (refusing
/// to silently ship an empty KB). Per-file read errors and per-item
/// store-write errors are logged as warnings and skipped, not fatal —
/// matching the existing binaries' behavior of getting as much content in
/// as possible rather than aborting on one bad node/link.
pub fn ingest_org_dir(
    store: &CozoKbStore,
    dir: &Path,
    source: NodeSource,
) -> Result<OrgIngestStats, KbBuildError> {
    if !dir.is_dir() {
        return Err(KbBuildError::MissingSourceDir(dir.to_path_buf()));
    }

    let mut all_nodes = Vec::new();
    let mut all_typed_links = Vec::new();
    let mut all_transclusions = Vec::new();

    let mut org_files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| KbBuildError::Io(dir.to_path_buf(), e))?
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
        all_nodes.extend(result.nodes);
        all_typed_links.extend(result.typed_links);
        all_transclusions.extend(result.transclusions);
    }

    let node_count = all_nodes.len();
    let typed_link_count = all_typed_links.len();
    let transclusion_count = all_transclusions.len();

    // Insert org-parsed nodes into the store (upsert, no delete). We use
    // insert_node rather than replace_all_nodes to avoid the CozoDB sled tombstone
    // issue: :rm leaves partial tuples that break load_all().
    insert_nodes(store, &all_nodes, source);
    eprintln!(
        "  Org files: {} files, {node_count} nodes parsed",
        org_files.len()
    );

    if node_count == 0 {
        return Err(KbBuildError::EmptyCorpus(dir.to_path_buf()));
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

    Ok(OrgIngestStats {
        org_files: org_files.len(),
        nodes: node_count,
        typed_links_parsed: typed_link_count,
        typed_links_stored: link_count,
        transclusions_parsed: transclusion_count,
        transclusions_stored: trans_count as usize,
    })
}

/// Error if `store` has no node with the literal id `"index"`.
///
/// `read_guidance_kb_context` (`crates/ai/src/guidance.rs`) looks up
/// exactly that id for whichever KB instance `ai_guidance_kb` names, so a
/// guidance KB shipped without one would silently fail to surface any
/// content at read time instead of failing loudly at build time.
///
/// `src_dir` names the caller's own corpus in the error, so each caller's
/// failure points at its own content rather than a generic location. It is
/// the same path that was ingested, rather than a separately-passed hint
/// string that could disagree with it.
pub fn require_index_node(store: &CozoKbStore, src_dir: &Path) -> Result<(), KbBuildError> {
    if store.get_node("index").ok().flatten().is_none() {
        return Err(KbBuildError::MissingIndexNode(src_dir.to_path_buf()));
    }
    Ok(())
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

        let stats = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

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

        let stats = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");
        assert_eq!(stats.org_files, 1);
        assert_eq!(stats.nodes, 1);
    }

    /// The provenance stamp is the thing that makes shipped content read-only:
    /// `kb_update_node_with` refuses `Some(NodeSource::Seed)` and nothing else.
    /// Assert it on a node read back out of the store, not on the input.
    #[test]
    fn ingested_nodes_carry_the_requested_provenance() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

        let node = store
            .get_node("test:a")
            .expect("query ok")
            .expect("node must exist");
        assert_eq!(
            node.source,
            Some(NodeSource::Seed),
            "an unstamped node is editable, and the next rebuild silently destroys the edit"
        );
    }

    /// The same guarantee for the generator that does *not* go through
    /// `ingest_org_dir` — the ADR KB builds `.md`-sourced nodes and inserts
    /// them directly, which is exactly how it ended up with `source == None`.
    #[test]
    fn insert_nodes_stamps_provenance_on_nodes_that_never_saw_an_org_file() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let unstamped = Node::new(
            "concept:adr-999-fixture",
            "ADR-999",
            crate::NodeKind::Concept,
            "Body.",
        );
        assert_eq!(unstamped.source, None, "fixture must start unstamped");

        let inserted = insert_nodes(&store, std::slice::from_ref(&unstamped), NodeSource::Seed);
        assert_eq!(inserted, 1);

        let read_back = store
            .get_node("concept:adr-999-fixture")
            .expect("query ok")
            .expect("node must exist");
        assert_eq!(read_back.source, Some(NodeSource::Seed));
    }

    #[test]
    fn ingest_org_dir_errors_on_missing_directory() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = ingest_org_dir(
            &store,
            Path::new("/nonexistent/definitely-not-a-real-dir-9f3c"),
            NodeSource::Seed,
        )
        .expect_err("a missing corpus must not silently produce an empty KB");
        assert!(matches!(err, KbBuildError::MissingSourceDir(_)), "{err:?}");
    }

    #[test]
    fn ingest_org_dir_errors_on_zero_nodes() {
        let src_dir = TempDir::new().unwrap();
        // Directory exists but has no .org files.
        std::fs::write(src_dir.path().join("README.md"), "nothing to ingest").unwrap();

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed)
            .expect_err("an empty corpus must not produce a shippable KB");
        assert!(matches!(err, KbBuildError::EmptyCorpus(_)), "{err:?}");
    }

    // --- require_index_node ---

    #[test]
    fn require_index_node_errors_on_fresh_store_with_no_index() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = require_index_node(&store, Path::new("assets/fixture-kb"))
            .expect_err("a guidance KB without an index node surfaces nothing at read time");
        assert!(matches!(err, KbBuildError::MissingIndexNode(_)), "{err:?}");
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
        ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

        require_index_node(&store, Path::new("assets/fixture-kb")).expect("index node present");
    }

    // --- build_org_kb ---

    /// The disk engine available in *this* build.
    ///
    /// `mae-kb`'s own default features are sled-only (`Cargo.toml`), so a bare
    /// `cargo test -p mae-kb` has no sqlite; the editor and daemon builds unify
    /// `storage-sqlite` in via `mae-core`/`daemon`. Selecting here rather than
    /// `#[cfg]`-ing the test away keeps the round-trip covered in both
    /// configurations — and the reason this matters at all is that the engine
    /// is a *runtime* string dispatch inside cozo: asking for an
    /// uncompiled engine fails with a `bail!` at open time, not at compile
    /// time, which is precisely why `open_fresh_store` takes it as a parameter.
    #[cfg(feature = "storage-sqlite")]
    const TEST_DISK_ENGINE: &str = "sqlite";
    #[cfg(not(feature = "storage-sqlite"))]
    const TEST_DISK_ENGINE: &str = "sled";

    /// The composed pipeline, built to disk and reopened independently.
    #[test]
    fn build_org_kb_produces_a_queryable_store_on_disk() {
        let src_dir = TempDir::new().unwrap();
        write_org(
            src_dir.path(),
            "index.org",
            ":PROPERTIES:\n:ID: index\n:END:\n#+title: Index\n\nEntry point.\n",
        );
        write_org(src_dir.path(), "a.org", NODE_A);

        let out = TempDir::new().unwrap();
        let output = out.path().join("fixture.cozo");
        let stats = build_org_kb(
            src_dir.path(),
            &output,
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect("build ok");
        assert_eq!(stats.nodes, 2);

        // Reopen independently: the point of building to disk is that another
        // process can read it back.
        let reopened = CozoKbStore::open_with_engine(&output, TEST_DISK_ENGINE).expect("reopen");
        let node = reopened
            .get_node("test:a")
            .expect("query ok")
            .expect("node must survive the round trip to disk");
        assert_eq!(node.title, "Node A");
        assert_eq!(node.source, Some(NodeSource::Seed));
    }

    #[test]
    fn build_org_kb_refuses_a_guidance_corpus_with_no_index_node() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);

        let out = TempDir::new().unwrap();
        let err = build_org_kb(
            src_dir.path(),
            &out.path().join("fixture.cozo"),
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect_err("require_index must be enforced by the composed pipeline too");
        assert!(matches!(err, KbBuildError::MissingIndexNode(_)), "{err:?}");
    }

    // --- open_fresh_store ---

    #[test]
    fn open_fresh_store_creates_parent_dirs_and_removes_stale_output() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("nested").join("dir").join("out.cozo");

        // First open: parent dirs don't exist yet.
        {
            let _store = open_fresh_store(&output, RELEASE_ASSET_ENGINE).expect("open ok");
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
            let _store = open_fresh_store(&output, RELEASE_ASSET_ENGINE).expect("open ok");
        }
        if output.is_dir() {
            assert!(
                !marker.exists(),
                "open_fresh_store must remove the prior store before reopening"
            );
        }
    }
}
