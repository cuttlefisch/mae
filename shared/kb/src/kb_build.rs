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

    remove_store_path(output_path)?;

    let store = CozoKbStore::open_with_engine(output_path, engine)
        .map_err(|e| KbBuildError::Store(e.to_string()))?;

    // Seed the relationship-type system (registry for type validation +
    // introspection; ADR-030 link parsing reads rel from each link's `?query`).
    store
        .seed_type_system()
        .map_err(|e| KbBuildError::Store(e.to_string()))?;

    Ok(store)
}

/// A sqlite store's WAL sidecars. Present only while a connection is open or
/// after an unclean close; a clean close checkpoints and removes them.
fn wal_sidecars(path: &Path) -> [std::path::PathBuf; 2] {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let mut shm = path.as_os_str().to_os_string();
    shm.push("-shm");
    [wal.into(), shm.into()]
}

/// Fold a sqlite store's WAL back into the main database and empty it, so the
/// store is a single self-contained file again.
///
/// Best-effort by design: a store that was never in WAL mode has nothing to
/// checkpoint, and a store that cannot be opened here is one the caller is about
/// to fail on anyway.
fn checkpoint_wal(path: &Path) {
    if !wal_sidecars(path).iter().any(|p| p.exists()) {
        return;
    }
    #[cfg(feature = "storage-sqlite")]
    if let Ok(conn) = sqlite::Connection::open(path) {
        // TRUNCATE (not PASSIVE) so the -wal is emptied rather than merely
        // checkpointed -- a non-empty -wal left beside a renamed database is the
        // failure this exists to prevent.
        let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    for side in wal_sidecars(path) {
        let _ = std::fs::remove_file(side);
    }
}

/// Delete whatever store currently sits at `path`, so a build starts fresh.
/// sled is a directory and sqlite a file, hence the branch. Absent is success.
///
/// @ai-caution: [storage] Must also remove the `-wal`/`-shm` sidecars. MAE puts
/// sqlite stores into WAL mode (`cozo_store::wal`), so a store is no longer a
/// single file — and a build that fails *before* a clean close leaves those
/// behind. Caught by `a_failed_build_leaves_no_store_and_no_staging_behind`,
/// which went red on macOS the moment WAL was enabled.
fn remove_store_path(path: &Path) -> Result<(), KbBuildError> {
    for side in wal_sidecars(path) {
        let _ = std::fs::remove_file(side);
    }
    if !path.exists() {
        return Ok(());
    }
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    removed.map_err(|e| KbBuildError::Io(path.to_path_buf(), e))
}

/// Sibling path a build is staged at before being promoted to `output_path`.
///
/// A sibling (not a tempdir) so the promoting `rename` stays within one
/// filesystem — a cross-device rename fails, and falling back to copy would
/// reintroduce the very partial-write window staging exists to remove. The pid
/// keeps two concurrent builders off each other's staging path.
fn staging_path(output_path: &Path) -> std::path::PathBuf {
    let name = output_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kb.cozo".to_string());
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.staging-{}", std::process::id()))
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
///
/// @ai-caution: [kb-provenance] The build is staged and then atomically renamed
/// into place — **`output_path` must never exist in a half-written state.**
/// Readers of a built store decide it is usable by testing `Path::exists()`
/// alone (`guidance::resolve_guidance_db_path`'s cache arm, and
/// `build_guidance_from_embedded_corpus`'s early return). Building in place made
/// that test a lie: the file appeared the instant the store opened and filled
/// over the following seconds, so a concurrent reader could open a store with no
/// `index` node yet and conclude the KB had no guidance. Worse, the early return
/// keys on the same check, so a build interrupted partway left a permanently
/// poisoned cache entry that was never rebuilt — guidance silently delivering
/// nothing until the version-keyed filename changed at the next release.
///
/// Staging makes existence mean completeness, which is what those callers
/// already assume. Do not "simplify" this back to building at `output_path`.
pub fn build_org_kb(
    src_dir: &Path,
    output_path: &Path,
    opts: &OrgKbBuildOptions,
) -> Result<OrgIngestStats, KbBuildError> {
    let staging = staging_path(output_path);
    let stats = build_into(src_dir, &staging, opts).inspect_err(|_| {
        // Never leave a failed build's staging store behind.
        let _ = remove_store_path(&staging);
    })?;

    // @ai-caution: [storage] `rename` moves ONE path, so a staged store whose
    // `-wal` still holds uncheckpointed pages would be published missing part of
    // its own contents. Checkpoint them INTO the main file first.
    //
    // An earlier version of this refused to promote when a sidecar was present,
    // on the reasoning that a clean close removes them so their presence means
    // something is wrong. That was too strict and broke the guidance-KB build on
    // macOS: whether a close unlinks the sidecars depends on platform timing and
    // on being the last connection, so their presence is normal, not a fault.
    // Refusing turned a recoverable state into a hard build failure.
    //
    // `wal_checkpoint(TRUNCATE)` is exactly the operation for this: it folds the
    // WAL back into the database and empties it, after which the sidecars carry
    // nothing and can be removed. Best-effort -- on a store that never went into
    // WAL there is nothing to do, and a failure here leaves the sidecars in place
    // where the check below still catches them.
    checkpoint_wal(&staging);

    // `rename` refuses an existing destination on Windows, so the old store has
    // to go first. That leaves a brief window where the path is ABSENT, which is
    // the safe direction: absent means "not built yet" to every reader, and they
    // rebuild or fall through. A partial store is what has to be impossible.
    remove_store_path(output_path)?;
    std::fs::rename(&staging, output_path).map_err(|e| {
        let _ = remove_store_path(&staging);
        KbBuildError::Io(output_path.to_path_buf(), e)
    })?;
    Ok(stats)
}

/// The build itself, against whatever path it is handed. Split out of
/// [`build_org_kb`] so the store is dropped — flushing sqlite's WAL and sled's
/// buffers — *before* the promoting rename runs, rather than at the end of the
/// caller's scope.
fn build_into(
    src_dir: &Path,
    path: &Path,
    opts: &OrgKbBuildOptions,
) -> Result<OrgIngestStats, KbBuildError> {
    let store = open_fresh_store(path, opts.engine)?;
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
#[path = "kb_build_tests.rs"]
mod kb_build_tests;
