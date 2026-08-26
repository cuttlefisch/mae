//! Story D / R8 — the import **plan**: assess before importing, and make the
//! preview binding rather than decorative.
//!
//! # Why this exists
//!
//! While `.org` files are truth, a partial import is an inconvenience — re-run
//! it. Once the store is truth (the cutover), the same partial import is **data
//! loss**, because the source is retired afterwards.
//!
//! The load-bearing evidence is obsidian-importer#547: a 7,460-note Evernote
//! import that lost **748 notes (~10%) silently**, with `skipped` and `failed`
//! both reading **zero** the whole time — the loss happened on paths that never
//! called the reporter. Obsidian already had the instrumentation. Counters
//! cannot audit the thing that produces them.
//!
//! [`ImportReport::unaccounted`](crate::federation::ImportReport::unaccounted)
//! already reconciles an independent census *after* the fact. This module is the
//! half that runs *before* it, plus the honest answer to the dry-run question.
//!
//! # The dry-run decision, stated rather than implied
//!
//! **A preview that is not binding is theatre.** Every notes-app importer
//! surveyed ships a preview that is *scoping* — it shows what would be looked at
//! and the import is then free to walk a different set. Only Terraform closes
//! the loop: `plan -out=FILE` produces the artifact `apply` consumes, so apply
//! **cannot** diverge from plan.
//!
//! MAE takes Terraform's shape. [`ImportPlan`] is a persisted manifest, and
//! [`ImportPlan::verify_source_unchanged`] refuses the import if the corpus moved
//! underneath it. A plan the user read and approved is the file set that gets
//! imported, or the import does not run.
//!
//! # `Failed` is not `Skipped`
//!
//! AWS DMS's validation vocabulary is explicit that an error condition must never
//! be able to present as a pass — which is #547's bug encoded as a status
//! vocabulary. [`FileDisposition`] keeps them apart: a file with no `:ID:` is
//! **explained** (skipped), a file that could not be read is **failed**, and the
//! two never collapse into one "not imported" bucket.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the assessment expects to happen to one source file.
///
/// `Skipped` and `Failed` are deliberately distinct: the first is a file the
/// importer understands and has a reason for, the second is a file it could not
/// process. Collapsing them is exactly how a ~10% loss reads as a clean run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDisposition {
    /// Parses, carries at least one node, and will be imported.
    Import { node_ids: Vec<String> },
    /// Deliberately not imported, with the reason — an *explained* non-import.
    Skipped { reason: String },
    /// Could not be read or parsed. **Not** a skip.
    Failed { reason: String },
}

impl FileDisposition {
    pub fn is_failure(&self) -> bool {
        matches!(self, FileDisposition::Failed { .. })
    }
}

/// One source file as the assessment found it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    pub path: PathBuf,
    /// SHA-256 of the file's bytes at assessment time. This is what makes the
    /// plan binding — see [`ImportPlan::verify_source_unchanged`].
    pub content_hash: String,
    pub disposition: FileDisposition,
}

/// A condition the operator should see **before** the import runs, not
/// discovered afterwards in a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportHazard {
    /// The same `:ID:` in more than one file. Whichever loses is silent loss.
    DuplicateId { id: String, paths: Vec<PathBuf> },
    /// Paths that differ only by case.
    ///
    /// **This is #547's headline bug and it cannot reproduce on Linux** —
    /// `Sales.md` silently overwritten by a note titled `sales`. MAE develops on
    /// macOS and Linux simultaneously (principle #13), so the hazard is detected
    /// by *comparison*, on every platform, rather than by waiting for a
    /// case-insensitive filesystem to demonstrate it.
    CaseFoldCollision { paths: Vec<PathBuf> },
    /// Paths whose bytes differ but whose Unicode normalization does not —
    /// the same trap as case folding, via NFC/NFD. macOS's HFS+/APFS historically
    /// normalize; Linux does not.
    UnicodeCollision { paths: Vec<PathBuf> },
    /// A file the assessment could not read.
    Unreadable { path: PathBuf, reason: String },
}

impl ImportHazard {
    /// One line, naming the files, because a hazard the operator cannot locate
    /// is not actionable.
    pub fn describe(&self) -> String {
        match self {
            ImportHazard::DuplicateId { id, paths } => {
                format!("duplicate :ID: {id} in {}", join_paths(paths))
            }
            ImportHazard::CaseFoldCollision { paths } => format!(
                "paths differ only by case (silently collides on macOS): {}",
                join_paths(paths)
            ),
            ImportHazard::UnicodeCollision { paths } => format!(
                "paths differ only by Unicode normalization: {}",
                join_paths(paths)
            ),
            ImportHazard::Unreadable { path, reason } => {
                format!("unreadable: {} ({reason})", path.display())
            }
        }
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The persisted manifest: what the import will consume, assessed without
/// mutating anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlan {
    pub source_root: PathBuf,
    pub files: Vec<PlannedFile>,
    pub hazards: Vec<ImportHazard>,
}

impl ImportPlan {
    /// Walk `org_dir` and describe what an import would do. **Reads only.**
    ///
    /// The source is never written, which is what makes it the undo: every
    /// importer surveyed offers no undo, and the universal answer is that the
    /// source *is* the undo because the import never touches it. Here that is
    /// structural rather than a promise.
    pub fn assess(org_dir: &Path) -> Self {
        let mut plan = ImportPlan {
            source_root: org_dir.to_path_buf(),
            files: Vec::new(),
            hazards: Vec::new(),
        };
        for path in walk_org_files(org_dir) {
            plan.files.push(assess_file(&path));
        }
        plan.hazards = detect_hazards(&plan.files);
        plan
    }

    /// Files that will produce nodes.
    pub fn importable(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.disposition, FileDisposition::Import { .. }))
            .count()
    }

    /// Files the assessment could not process. **Distinct from skipped.**
    pub fn failed(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.disposition.is_failure())
            .count()
    }

    /// Files deliberately not imported, each with a reason.
    pub fn skipped(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.disposition, FileDisposition::Skipped { .. }))
            .count()
    }

    /// Nodes the import is expected to produce.
    pub fn expected_nodes(&self) -> usize {
        self.files
            .iter()
            .filter_map(|f| match &f.disposition {
                FileDisposition::Import { node_ids } => Some(node_ids.len()),
                _ => None,
            })
            .sum()
    }

    /// The operator-facing summary. Every number that could hide a loss is on
    /// it, and failures are named separately from skips.
    pub fn summary(&self) -> String {
        format!(
            "{} file(s) → {} node(s); {} skipped, {} failed, {} hazard(s)",
            self.files.len(),
            self.expected_nodes(),
            self.skipped(),
            self.failed(),
            self.hazards.len()
        )
    }

    /// **The binding check.** Refuse the import if the corpus changed after the
    /// plan the operator approved was written.
    ///
    /// Terraform's `plan -out` / `apply` contract, and the reason this module
    /// exists rather than a `--dry-run` flag: a preview that the subsequent run
    /// is free to diverge from tells the operator nothing they can rely on.
    ///
    /// Returns the drift, or `Ok(())` when the plan still describes the corpus.
    pub fn verify_source_unchanged(&self) -> Result<(), Vec<PlanDrift>> {
        let mut drift = Vec::new();
        let current: HashMap<PathBuf, String> = walk_org_files(&self.source_root)
            .into_iter()
            .map(|p| {
                let h = hash_file(&p).unwrap_or_else(|| "<unreadable>".to_string());
                (p, h)
            })
            .collect();

        for planned in &self.files {
            match current.get(&planned.path) {
                None => drift.push(PlanDrift::Vanished(planned.path.clone())),
                Some(h) if *h != planned.content_hash => {
                    drift.push(PlanDrift::Modified(planned.path.clone()))
                }
                Some(_) => {}
            }
        }
        let planned_paths: std::collections::HashSet<&PathBuf> =
            self.files.iter().map(|f| &f.path).collect();
        for path in current.keys() {
            if !planned_paths.contains(path) {
                drift.push(PlanDrift::Appeared(path.clone()));
            }
        }

        if drift.is_empty() {
            Ok(())
        } else {
            Err(drift)
        }
    }
}

/// How the corpus moved between plan and apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDrift {
    /// A planned file is gone — importing now silently imports less.
    Vanished(PathBuf),
    /// A planned file's content changed — the node ids previewed may be wrong.
    Modified(PathBuf),
    /// A file the operator never saw in the preview.
    Appeared(PathBuf),
}

impl PlanDrift {
    pub fn describe(&self) -> String {
        match self {
            PlanDrift::Vanished(p) => format!("vanished since the plan: {}", p.display()),
            PlanDrift::Modified(p) => format!("modified since the plan: {}", p.display()),
            PlanDrift::Appeared(p) => format!("appeared since the plan: {}", p.display()),
        }
    }
}

/// A stable, collision-resistant key for "the plan for THIS source directory".
///
/// Callers must pass a `realpath`-canonicalized path: the same corpus reached
/// through a symlink must not mint a second plan, and two different corpora must
/// not share one. Deriving the key from an un-canonicalized path is the VS Code
/// `workspaceStorage` mistake, and it is unfixable once state is keyed on it.
pub fn plan_key(canonical_source: &str) -> String {
    hex::encode(Sha256::digest(canonical_source.as_bytes()))[..16].to_string()
}

fn walk_org_files(org_dir: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(org_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("org"))
        .collect()
}

fn hash_file(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| hex::encode(Sha256::digest(&b)))
}

fn assess_file(path: &Path) -> PlannedFile {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return PlannedFile {
                path: path.to_path_buf(),
                content_hash: String::new(),
                disposition: FileDisposition::Failed {
                    reason: e.to_string(),
                },
            }
        }
    };
    let content_hash = hex::encode(Sha256::digest(&bytes));
    let disposition = disposition_for(path, bytes);
    PlannedFile {
        path: path.to_path_buf(),
        content_hash,
        disposition,
    }
}

fn disposition_for(path: &Path, bytes: Vec<u8>) -> FileDisposition {
    if path.file_name().and_then(|n| n.to_str()) == Some("eor-instance.org") {
        return FileDisposition::Skipped {
            reason: "instance sentinel file".to_string(),
        };
    }
    let content = match String::from_utf8(bytes) {
        Ok(c) => c,
        Err(e) => {
            return FileDisposition::Failed {
                reason: format!("not valid UTF-8: {e}"),
            }
        }
    };
    let nodes = crate::org::parse_org_multi(&content);
    if nodes.is_empty() {
        return FileDisposition::Skipped {
            reason: "no node with an :ID: property".to_string(),
        };
    }
    FileDisposition::Import {
        node_ids: nodes.into_iter().map(|n| n.id).collect(),
    }
}

fn detect_hazards(files: &[PlannedFile]) -> Vec<ImportHazard> {
    let mut hazards = collision_hazards(files);
    hazards.extend(duplicate_id_hazards(files));
    for f in files {
        if let FileDisposition::Failed { reason } = &f.disposition {
            hazards.push(ImportHazard::Unreadable {
                path: f.path.clone(),
                reason: reason.clone(),
            });
        }
    }
    hazards
}

fn duplicate_id_hazards(files: &[PlannedFile]) -> Vec<ImportHazard> {
    let mut by_id: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    for f in files {
        if let FileDisposition::Import { node_ids } = &f.disposition {
            for id in node_ids {
                by_id.entry(id).or_default().push(f.path.clone());
            }
        }
    }
    let mut out: Vec<ImportHazard> = by_id
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(id, paths)| ImportHazard::DuplicateId {
            id: id.to_string(),
            paths,
        })
        .collect();
    out.sort_by_key(|h| h.describe());
    out
}

/// Case-fold and Unicode-normalization collisions, detected by comparison so
/// they are found on **both** platforms rather than only where the filesystem
/// demonstrates them (principle #13).
fn collision_hazards(files: &[PlannedFile]) -> Vec<ImportHazard> {
    let mut by_folded: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut by_normalized: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for f in files {
        let s = f.path.to_string_lossy().to_string();
        by_folded
            .entry(s.to_lowercase())
            .or_default()
            .push(f.path.clone());
        by_normalized
            .entry(nfc(&s))
            .or_default()
            .push(f.path.clone());
    }
    let mut out = Vec::new();
    for (_, paths) in by_folded.into_iter().filter(|(_, p)| p.len() > 1) {
        out.push(ImportHazard::CaseFoldCollision { paths });
    }
    for (_, paths) in by_normalized.into_iter().filter(|(_, p)| p.len() > 1) {
        let already = out.iter().any(|h| match h {
            ImportHazard::CaseFoldCollision { paths: cp } => *cp == paths,
            _ => false,
        });
        if !already {
            out.push(ImportHazard::UnicodeCollision { paths });
        }
    }
    out.sort_by_key(|h| h.describe());
    out
}

/// Minimal NFC folding for the collision check.
///
/// Deliberately **not** a full Unicode normalization implementation — `mae-kb`
/// carries no `unicode-normalization` dependency, and this is a *detector*: its
/// job is to raise a hazard for the operator to look at, so a false negative on
/// an exotic sequence costs a missed warning, not a wrong import. It folds the
/// combining-mark forms that actually appear in filenames from macOS (NFD) —
/// a base character followed by a combining diacritic — onto their precomposed
/// counterparts.
fn nfc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let Some(&next) = chars.peek() else {
            out.push(c);
            continue;
        };
        match precompose(c, next) {
            Some(combined) => {
                out.push(combined);
                chars.next();
            }
            None => out.push(c),
        }
    }
    out
}

fn precompose(base: char, mark: char) -> Option<char> {
    // U+0300..U+036F is the combining-diacritical-marks block.
    if !('\u{0300}'..='\u{036F}').contains(&mark) {
        return None;
    }
    const PAIRS: &[(char, char, char)] = &[
        ('a', '\u{0301}', 'á'),
        ('e', '\u{0301}', 'é'),
        ('i', '\u{0301}', 'í'),
        ('o', '\u{0301}', 'ó'),
        ('u', '\u{0301}', 'ú'),
        ('n', '\u{0303}', 'ñ'),
        ('a', '\u{0308}', 'ä'),
        ('o', '\u{0308}', 'ö'),
        ('u', '\u{0308}', 'ü'),
        ('c', '\u{0327}', 'ç'),
    ];
    PAIRS
        .iter()
        .find(|(b, m, _)| *b == base && *m == mark)
        .map(|(_, _, c)| *c)
}

// ---------------------------------------------------------------------------
// Persistence — "results persisted", and the artifact the apply step consumes.
// ---------------------------------------------------------------------------

impl ImportPlan {
    /// Write the plan so a later import can be held to it.
    ///
    /// R8's pre-flight assessment is only useful if it outlives the terminal
    /// scrollback it was printed to — and the binding contract above needs a
    /// file to bind *to*.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

// ---------------------------------------------------------------------------
// The repeatable reconciliation pass — mutates nothing, re-runnable later.
// ---------------------------------------------------------------------------

/// How one source file's nodes compare against the destination, in the
/// vocabulary three unrelated systems converged on independently — AWS DMS
/// validation, `rclone check`, and `rsync --itemize-changes`.
///
/// Convergent naming is the point: it is the vocabulary that survived contact
/// with production in three separate tools, so it is the one an operator has
/// most likely already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    /// Present in both, and the destination holds every node the source names.
    Match { path: PathBuf, nodes: usize },
    /// In the source, absent from the destination — **the loss case**.
    SourceOnly {
        path: PathBuf,
        node_ids: Vec<String>,
    },
    /// The file's nodes are only partly present.
    Differ {
        path: PathBuf,
        present: usize,
        missing: Vec<String>,
    },
    /// The source could not be read, so nothing can be concluded. Never a pass.
    Error { path: PathBuf, reason: String },
}

/// The result of re-running the reconciliation. Safe to run at any time; it
/// reads both sides and writes neither.
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub rows: Vec<Reconciliation>,
}

impl ReconcileReport {
    pub fn matched(&self) -> usize {
        self.count(|r| matches!(r, Reconciliation::Match { .. }))
    }
    pub fn source_only(&self) -> usize {
        self.count(|r| matches!(r, Reconciliation::SourceOnly { .. }))
    }
    pub fn differing(&self) -> usize {
        self.count(|r| matches!(r, Reconciliation::Differ { .. }))
    }
    pub fn errored(&self) -> usize {
        self.count(|r| matches!(r, Reconciliation::Error { .. }))
    }

    fn count(&self, f: impl Fn(&Reconciliation) -> bool) -> usize {
        self.rows.iter().filter(|r| f(r)).count()
    }

    /// **An error must never be able to present as a pass** (AWS DMS states this
    /// explicitly; it is obsidian-importer#547's bug written as a status
    /// vocabulary). So "clean" requires the error count to be zero as well —
    /// an unreadable source file is an unknown, not an absence.
    pub fn is_clean(&self) -> bool {
        self.source_only() == 0 && self.differing() == 0 && self.errored() == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "{} match, {} source-only, {} differ, {} error",
            self.matched(),
            self.source_only(),
            self.differing(),
            self.errored()
        )
    }
}

/// Re-assess the source and compare it against whatever the destination holds.
///
/// `destination_has` answers "is this node id in the destination?" — supplied by
/// the caller so this works against an in-memory [`crate::KnowledgeBase`], a
/// Cozo store, or a remote query layer without this module knowing which.
pub fn reconcile(org_dir: &Path, destination_has: &dyn Fn(&str) -> bool) -> ReconcileReport {
    let plan = ImportPlan::assess(org_dir);
    let rows = plan
        .files
        .iter()
        .filter_map(|f| row_for(f, destination_has));
    ReconcileReport {
        rows: rows.collect(),
    }
}

fn row_for(f: &PlannedFile, has: &dyn Fn(&str) -> bool) -> Option<Reconciliation> {
    match &f.disposition {
        FileDisposition::Skipped { .. } => None,
        FileDisposition::Failed { reason } => Some(Reconciliation::Error {
            path: f.path.clone(),
            reason: reason.clone(),
        }),
        FileDisposition::Import { node_ids } => {
            let missing: Vec<String> = node_ids.iter().filter(|id| !has(id)).cloned().collect();
            Some(classify(f, node_ids, missing))
        }
    }
}

fn classify(f: &PlannedFile, node_ids: &[String], missing: Vec<String>) -> Reconciliation {
    if missing.is_empty() {
        Reconciliation::Match {
            path: f.path.clone(),
            nodes: node_ids.len(),
        }
    } else if missing.len() == node_ids.len() {
        Reconciliation::SourceOnly {
            path: f.path.clone(),
            node_ids: missing,
        }
    } else {
        Reconciliation::Differ {
            path: f.path.clone(),
            present: node_ids.len() - missing.len(),
            missing,
        }
    }
}

// ---------------------------------------------------------------------------
// The loss report, written INTO the destination.
// ---------------------------------------------------------------------------

/// Node id of the durable loss report. Stable, so a re-run replaces the previous
/// one rather than littering the KB with a growing pile of timestamped notes.
pub const LOSS_REPORT_ID: &str = "meta:import-loss-report";

/// Build the per-item loss report as a **node in the destination KB**.
///
/// R8's fourth layer, and the one every importer skips: per-item failures in a
/// durable, queryable artifact stored **in the destination**. Not a toast, not a
/// console line, not a log file the user will never open. Obsidian writes a
/// report note into the vault; MAE's destination is a KB, so the MAE-native form
/// of the same idea is a node — which additionally makes it reachable by
/// `kb_search`/`kb_get` and by the agent, rather than only by a human who
/// happened to scroll.
///
/// The census line leads, because it is the number that is *not* self-reported
/// by the importer.
pub fn loss_report_node(
    plan: &ImportPlan,
    report: &crate::federation::ImportReport,
) -> crate::Node {
    let mut body = String::new();
    body.push_str("* Import reconciliation\n\n");
    body.push_str(&format!("{}\n\n", report.census_line()));
    body.push_str(&format!("Plan: {}\n\n", plan.summary()));
    append_section(
        &mut body,
        "Unaccounted (no counter explains these)",
        unaccounted_lines(report),
    );
    append_section(&mut body, "Failed", failed_lines(plan));
    append_section(&mut body, "Skipped (explained)", skipped_lines(plan));
    append_section(
        &mut body,
        "Hazards",
        plan.hazards.iter().map(|h| h.describe()).collect(),
    );
    append_section(
        &mut body,
        "Duplicate ids seen during import",
        report
            .duplicate_ids
            .iter()
            .map(|(id, p)| format!("{id} — {}", p.display()))
            .collect(),
    );

    let mut node = crate::Node::new(
        LOSS_REPORT_ID,
        "Import loss report",
        crate::NodeKind::Meta,
        body,
    );
    node.properties
        .insert("unaccounted".to_string(), report.unaccounted().to_string());
    node.properties.insert(
        "source-files".to_string(),
        report.source_files_seen.to_string(),
    );
    node.properties
        .insert("failed".to_string(), plan.failed().to_string());
    node
}

fn unaccounted_lines(report: &crate::federation::ImportReport) -> Vec<String> {
    report
        .unaccounted_files
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}

fn failed_lines(plan: &ImportPlan) -> Vec<String> {
    plan.files
        .iter()
        .filter_map(|f| match &f.disposition {
            FileDisposition::Failed { reason } => Some(format!("{} — {reason}", f.path.display())),
            _ => None,
        })
        .collect()
}

fn skipped_lines(plan: &ImportPlan) -> Vec<String> {
    plan.files
        .iter()
        .filter_map(|f| match &f.disposition {
            FileDisposition::Skipped { reason } => Some(format!("{} — {reason}", f.path.display())),
            _ => None,
        })
        .collect()
}

/// An empty section still prints, with "none" — because a **missing** section
/// and a section reporting zero read identically to a human, and the whole point
/// of this artifact is that "nothing was lost" must be distinguishable from
/// "nothing was checked".
fn append_section(body: &mut String, title: &str, lines: Vec<String>) {
    body.push_str(&format!("** {title} ({})\n", lines.len()));
    if lines.is_empty() {
        body.push_str("none\n\n");
        return;
    }
    for line in lines {
        body.push_str(&format!("- {line}\n"));
    }
    body.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    fn node_file(id: &str, title: &str) -> String {
        format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {title}\n\nbody text\n")
    }

    // -- the pre-flight assessment ------------------------------------------

    /// The assessment must **read only**. The source is the undo; that has to be
    /// structural, not a promise, because every importer surveyed offers no undo
    /// and relies on exactly this property.
    #[test]
    fn assessment_does_not_touch_the_source() {
        let d = TempDir::new().unwrap();
        let f = write(d.path(), "a.org", &node_file("n1", "A"));
        let before = std::fs::read(&f).unwrap();
        let mtime_before = std::fs::metadata(&f).unwrap().modified().unwrap();

        let plan = ImportPlan::assess(d.path());

        assert_eq!(plan.importable(), 1);
        assert_eq!(
            std::fs::read(&f).unwrap(),
            before,
            "content must be untouched"
        );
        assert_eq!(
            std::fs::metadata(&f).unwrap().modified().unwrap(),
            mtime_before,
            "the source must not even be rewritten identically"
        );
    }

    /// **`Failed` is not `Skipped`.** AWS DMS is explicit that an error condition
    /// must never present as a pass; collapsing the two is obsidian-importer#547
    /// encoded as a status vocabulary.
    #[test]
    fn a_file_with_no_id_is_skipped_but_an_unreadable_one_is_failed() {
        let d = TempDir::new().unwrap();
        write(d.path(), "good.org", &node_file("n1", "A"));
        write(d.path(), "prose.org", "just prose, no ID drawer\n");
        std::fs::write(d.path().join("binary.org"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();

        let plan = ImportPlan::assess(d.path());

        assert_eq!(plan.importable(), 1);
        assert_eq!(
            plan.skipped(),
            1,
            "the ID-less file is EXPLAINED, not failed"
        );
        assert_eq!(
            plan.failed(),
            1,
            "the unreadable file must not read as a skip"
        );
        assert!(
            plan.summary().contains("1 failed"),
            "the summary must name failures separately: {}",
            plan.summary()
        );
    }

    /// A planned file whose node id is derived from its path, so two fixtures
    /// never collide on `:ID:` by accident — the negative-control test below
    /// caught exactly that on the first attempt.
    fn planned(path: &str) -> PlannedFile {
        PlannedFile {
            path: PathBuf::from(path),
            content_hash: "h".to_string(),
            disposition: FileDisposition::Import {
                node_ids: vec![format!("id-for{path}")],
            },
        }
    }

    /// The hazard that lost 748 notes.
    ///
    /// **Driven from constructed paths, not from the filesystem, and that is the
    /// point.** On a case-insensitive filesystem — Windows NTFS, macOS APFS by
    /// default — `Sales.org` and `sales.org` *cannot both exist*: writing the
    /// second overwrites the first, which is the data loss itself, happening
    /// before MAE ever sees it. A test that created both files would therefore
    /// pass on Linux and fail everywhere the bug actually bites. (It did: CI's
    /// Windows leg caught exactly that on the first push.)
    ///
    /// The detector's contract is over the path SET, so the test is too
    /// (principle #13 — one platform's filesystem behaviour is not a property).
    #[test]
    fn case_folding_collisions_are_detected_by_comparison_not_by_filesystem() {
        let files = [planned("/kb/Sales.org"), planned("/kb/sales.org")];

        let hazards = detect_hazards(&files);

        assert!(
            hazards
                .iter()
                .any(|h| matches!(h, ImportHazard::CaseFoldCollision { .. })),
            "hazards were {hazards:?}"
        );
    }

    /// Paths that differ only by case are a collision; paths that differ by more
    /// than case are not. Without this the detector could pass the test above by
    /// flagging everything.
    #[test]
    fn ordinary_distinct_paths_raise_no_collision_hazard() {
        let files = [planned("/kb/sales.org"), planned("/kb/marketing.org")];

        let hazards = detect_hazards(&files);

        assert!(hazards.is_empty(), "hazards were {hazards:?}");
    }

    /// The NFC/NFD twin of the case-fold trap: macOS historically normalizes
    /// filenames, Linux does not. Same reasoning as above for driving it from
    /// constructed paths.
    #[test]
    fn unicode_normalization_collisions_are_detected() {
        let files = [planned("/kb/cafe\u{0301}.org"), planned("/kb/café.org")];

        let hazards = detect_hazards(&files);

        assert!(
            hazards
                .iter()
                .any(|h| matches!(h, ImportHazard::UnicodeCollision { .. })),
            "hazards were {hazards:?}"
        );
    }

    /// Two files claiming one `:ID:` means one of them loses, silently.
    #[test]
    fn a_duplicate_id_across_files_is_a_hazard_before_the_import_runs() {
        let d = TempDir::new().unwrap();
        write(d.path(), "one.org", &node_file("dup", "One"));
        write(d.path(), "two.org", &node_file("dup", "Two"));

        let plan = ImportPlan::assess(d.path());

        let hazard = plan
            .hazards
            .iter()
            .find(|h| matches!(h, ImportHazard::DuplicateId { .. }))
            .unwrap_or_else(|| panic!("hazards were {:?}", plan.hazards));
        assert!(hazard.describe().contains("dup"));
    }

    // -- the binding contract ------------------------------------------------

    /// **The dry-run decision.** A preview the apply step may diverge from is
    /// theatre; Terraform's `plan -out`/`apply` is the only shape in the field
    /// that closes the loop. Three drifts, each of which silently changes what
    /// gets imported.
    #[test]
    fn a_plan_is_binding_and_names_every_way_the_source_moved() {
        let d = TempDir::new().unwrap();
        write(d.path(), "keep.org", &node_file("k", "Keep"));
        let edited = write(d.path(), "edit.org", &node_file("e", "Edit"));
        let removed = write(d.path(), "gone.org", &node_file("g", "Gone"));

        let plan = ImportPlan::assess(d.path());
        assert!(
            plan.verify_source_unchanged().is_ok(),
            "fresh plan must verify"
        );

        std::fs::write(&edited, node_file("e", "Edited after the plan")).unwrap();
        std::fs::remove_file(&removed).unwrap();
        write(d.path(), "new.org", &node_file("n", "New"));

        let drift = plan.verify_source_unchanged().unwrap_err();
        let described: Vec<String> = drift.iter().map(|d| d.describe()).collect();

        assert!(
            drift.contains(&PlanDrift::Modified(edited)),
            "{described:?}"
        );
        assert!(
            drift.contains(&PlanDrift::Vanished(removed)),
            "{described:?}"
        );
        assert!(
            drift.iter().any(|d| matches!(d, PlanDrift::Appeared(_))),
            "a file the operator never saw in the preview must be named: {described:?}"
        );
    }

    /// The plan has to outlive the terminal it was printed to, or the binding
    /// contract has nothing to bind to.
    #[test]
    fn a_plan_round_trips_through_disk() {
        let d = TempDir::new().unwrap();
        write(d.path(), "a.org", &node_file("n1", "A"));
        let plan = ImportPlan::assess(d.path());

        let path = d.path().join(".mae").join("import-plan.json");
        plan.save(&path).unwrap();
        let loaded = ImportPlan::load(&path).unwrap();

        assert_eq!(loaded.files.len(), plan.files.len());
        assert_eq!(loaded.expected_nodes(), plan.expected_nodes());
        assert!(
            loaded.verify_source_unchanged().is_ok(),
            "a reloaded plan must still bind the same corpus"
        );
    }

    // -- the repeatable reconciliation pass ----------------------------------

    /// Re-runnable, and it reads both sides rather than trusting the importer's
    /// own bookkeeping.
    #[test]
    fn reconciliation_separates_missing_from_partially_present() {
        let d = TempDir::new().unwrap();
        write(d.path(), "all.org", &node_file("a1", "All"));
        write(d.path(), "none.org", &node_file("b1", "None"));
        // A genuinely multi-node file needs HEADINGS: a second file-level
        // drawer does not make a second node, it makes one node with a
        // confused header. Verified against `parse_org_multi`, not assumed.
        write(
            d.path(),
            "part.org",
            "* C1\n:PROPERTIES:\n:ID: c1\n:END:\nbody\n\n             * C2\n:PROPERTIES:\n:ID: c2\n:END:\nbody\n",
        );

        let present = ["a1", "c1"];
        let has = |id: &str| present.contains(&id);
        let r = reconcile(d.path(), &has);

        assert_eq!(r.matched(), 1, "{:?}", r.rows);
        assert_eq!(r.source_only(), 1, "a wholly-absent file is the LOSS case");
        assert_eq!(r.differing(), 1, "a partly-present file is not a match");
        assert!(!r.is_clean());
    }

    /// **An unreadable source is an unknown, not an absence.** DMS's rule, and
    /// the one that keeps #547's shape from recurring: a run with an internal
    /// error must not be able to report clean.
    #[test]
    fn an_errored_file_prevents_a_clean_verdict() {
        let d = TempDir::new().unwrap();
        write(d.path(), "good.org", &node_file("g", "Good"));
        std::fs::write(d.path().join("bad.org"), [0xff, 0xfe, 0x00]).unwrap();

        let r = reconcile(d.path(), &|id: &str| id == "g");

        assert_eq!(r.matched(), 1);
        assert_eq!(r.errored(), 1);
        assert!(
            !r.is_clean(),
            "an error must never present as a pass: {}",
            r.summary()
        );
    }

    /// A skipped file is explained, so it must not show up as loss — otherwise
    /// every corpus with a README reports permanent phantom loss and operators
    /// learn to ignore the report.
    #[test]
    fn an_explained_skip_is_not_reported_as_loss() {
        let d = TempDir::new().unwrap();
        write(d.path(), "prose.org", "no ID drawer here\n");
        write(d.path(), "eor-instance.org", "sentinel\n");

        let r = reconcile(d.path(), &|_| false);

        assert!(r.rows.is_empty(), "{:?}", r.rows);
        assert!(r.is_clean());
    }

    // -- the loss report, in the destination ---------------------------------

    /// Per-item, durable, queryable, and **in the destination** — not a toast.
    #[test]
    fn the_loss_report_is_a_node_naming_every_lost_item() {
        let d = TempDir::new().unwrap();
        write(d.path(), "good.org", &node_file("g", "Good"));
        std::fs::write(d.path().join("bad.org"), [0xff, 0xfe]).unwrap();
        let plan = ImportPlan::assess(d.path());

        let mut report = crate::federation::ImportReport {
            source_files_seen: 3,
            nodes_imported: 1,
            ..Default::default()
        };
        report.unaccounted_files = vec![PathBuf::from("/kb/vanished.org")];

        let node = loss_report_node(&plan, &report);

        assert_eq!(node.id, LOSS_REPORT_ID);
        assert!(node.body.contains("vanished.org"), "{}", node.body);
        assert!(node.body.contains("bad.org"), "{}", node.body);
        assert_eq!(
            node.properties.get("unaccounted").map(String::as_str),
            Some("1")
        );
    }

    /// **"Nothing was lost" must be distinguishable from "nothing was checked".**
    /// A section that vanishes when empty reads as the second while claiming the
    /// first — which is the exact way #547's zeroes were believed.
    #[test]
    fn a_clean_report_still_prints_every_section_as_none() {
        let d = TempDir::new().unwrap();
        write(d.path(), "good.org", &node_file("g", "Good"));
        let plan = ImportPlan::assess(d.path());
        let report = crate::federation::ImportReport {
            source_files_seen: 1,
            nodes_imported: 1,
            ..Default::default()
        };

        let node = loss_report_node(&plan, &report);

        assert!(node.body.contains("Unaccounted"), "{}", node.body);
        assert!(node.body.contains("Failed (0)"), "{}", node.body);
        assert!(node.body.contains("none"), "{}", node.body);
        assert!(node.body.contains("0 unaccounted"), "{}", node.body);
    }
}
