//! Building a [`SeedPlan`]: selection, per-file split, cross-file checks and the
//! plan hash. [`plan`] reads git; [`plan_entries`] is the same logic over
//! entries and blob bytes already in hand, so it is testable without a repo.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::git::{GitRepo, TreeEntry};
use super::glob::Glob;
use super::history::{self, History};
use super::split::{regen, split, SplitFile};
use super::{Problem, ProblemKind, SeedError, CONTRACT, DEFAULT_INCLUDE};

/// What to plan.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub repo: PathBuf,
    pub rev: String,
    /// `--include` globs. Empty means [`DEFAULT_INCLUDE`].
    pub include: Vec<String>,
    /// `--exclude` paths: exact repo-relative paths inside the selection.
    pub exclude: Vec<String>,
    pub max_body_bytes: usize,
    pub require_canonical: bool,
    /// `--code-property`: the drawer key holding a page's document code
    /// (ADR-113 D2). `None`: no codes are reported or required.
    pub code_property: Option<String>,
}

impl PlanOptions {
    /// The patterns actually in force, defaults applied.
    pub fn effective_include(&self) -> Vec<String> {
        if self.include.is_empty() {
            vec![DEFAULT_INCLUDE.to_string()]
        } else {
            self.include.clone()
        }
    }
}

/// The commit a plan was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub repo: String,
    pub rev: String,
    pub commit: String,
    pub tree: String,
}

/// A checkout-affecting attribute on a selected path (`git::CHECKOUT_ATTRS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutAttr {
    pub path: String,
    pub attr: String,
    pub value: String,
}

/// Everything [`plan_entries`] needs besides the options: the tree, the blob
/// bytes by sha, and the checkout attributes of the selected paths.
#[derive(Debug, Clone)]
pub struct PlanInput {
    pub source: Source,
    pub entries: Vec<TreeEntry>,
    pub blobs: HashMap<String, Vec<u8>>,
    /// Blob sizes from `cat-file --batch-check`, by sha. A selected blob over
    /// [`read_limit`] has no bytes in `blobs` and is refused from its size.
    pub sizes: HashMap<String, u64>,
    pub attrs: Vec<CheckoutAttr>,
    /// The history walk: path lineages and the first appearance of each id.
    pub history: History,
    /// When the plan was made: both times of a file git has no history for.
    pub received_at: i64,
}

/// Where a node's time came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    /// The first appearance of the node's `:ID:` in history (creation only).
    GitId,
    /// The file's path lineage at the rev.
    GitPath,
    /// No history: the time the plan was made. Not reproducible.
    Received,
}

impl TimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TimeSource::GitId => "git-id",
            TimeSource::GitPath => "git-path",
            TimeSource::Received => "received",
        }
    }
}

/// One file that would become one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedNode {
    pub path: String,
    pub blob: String,
    pub file: SplitFile,
    /// `F == N(F)`.
    pub canonical: bool,
    /// For a non-canonical file: the lines of F and of N(F) that differ
    /// (common leading and trailing lines removed), each with its `\n`.
    pub header_diff: Option<(Vec<String>, Vec<String>)>,
    /// Creation (unix seconds): the first commit in which the node's `:ID:`
    /// appeared, at any path (`history`).
    pub created_at: i64,
    pub created_source: TimeSource,
    /// Last modification: the last commit that changed the file's own path
    /// lineage at the rev.
    pub modified_at: i64,
    pub modified_source: TimeSource,
    /// The value of `--code-property` in the file drawer, if set and non-empty.
    pub code: Option<String>,
}

/// A reason the plan cannot be applied, attached to a path (`""` for the run).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Refusal {
    pub path: String,
    pub kind: ProblemKind,
    pub detail: String,
}

/// The outcome of planning. Applying is allowed only when `refusals` is empty.
#[derive(Debug, Clone)]
pub struct SeedPlan {
    pub source: Source,
    pub include: Vec<String>,
    pub max_body_bytes: usize,
    pub require_canonical: bool,
    pub code_property: Option<String>,
    /// Sorted by path.
    pub nodes: Vec<PlannedNode>,
    /// Selected paths removed by `--exclude`, sorted. Never silent: the report
    /// lists every one, and apply will record them in the seed provenance.
    pub excluded: Vec<String>,
    /// Paths in the tree that no `--include` matches, sorted.
    pub not_selected: Vec<String>,
    /// Sorted by path, then kind.
    pub refusals: Vec<Refusal>,
    /// With `--code-property`: the planned nodes that have no code. Listed,
    /// not refused (no document codes exist yet).
    pub missing_code: Vec<String>,
    /// How many history blobs were read to date the ids (the walk's cost).
    pub history_blobs_read: usize,
}

impl SeedPlan {
    pub fn ok(&self) -> bool {
        self.refusals.is_empty()
    }

    /// The identity of what would be seeded: a SHA-256 over the contract and
    /// every node's fields and body, in path order. Equal for two plans of the
    /// same content under the same contract; `None` for a refused plan, so a
    /// hash is never mistaken for a seedable one.
    pub fn plan_hash(&self) -> Option<String> {
        self.ok().then(|| plan_hash(&self.nodes))
    }

    pub fn anchor_count(&self) -> usize {
        self.nodes.iter().map(|n| n.file.anchors.len()).sum()
    }
}

/// Plan the seed of `opts.rev` in `opts.repo`. Reads git only.
pub fn plan(opts: &PlanOptions) -> Result<SeedPlan, SeedError> {
    let globs = parse_globs(opts)?;
    let repo = GitRepo::new(&opts.repo);
    repo.preflight()?;
    let (commit, tree) = repo.resolve(&opts.rev)?;
    let entries = repo.ls_tree(&commit)?;
    let selection = select(&entries, &globs, &opts.exclude);
    let paths: Vec<String> = selection.selected.iter().map(|(p, _)| p.clone()).collect();
    let shas: Vec<String> = selection.selected.iter().map(|(_, s)| s.clone()).collect();
    let attrs = repo
        .checkout_attributes(&commit, &paths)?
        .into_iter()
        .map(|(path, attr, value)| CheckoutAttr { path, attr, value })
        .collect();
    // Sizes first: a blob too large to be a page is refused unread.
    let sizes: HashMap<String, u64> = shas.iter().cloned().zip(repo.blob_sizes(&shas)?).collect();
    let limit = read_limit(opts);
    let readable: Vec<String> = shas
        .iter()
        .filter(|s| sizes[*s] <= limit)
        .cloned()
        .collect();
    let blobs = readable
        .iter()
        .cloned()
        .zip(repo.read_blobs(&readable)?)
        .collect();
    let history = history::history(&repo, &commit)?;
    let received_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let source = Source {
        repo: opts.repo.display().to_string(),
        rev: opts.rev.clone(),
        commit,
        tree,
    };
    plan_entries(
        PlanInput {
            source,
            entries,
            blobs,
            sizes,
            attrs,
            history,
            received_at,
        },
        opts,
    )
}

/// [`plan`] over a tree and its blobs already in hand.
pub fn plan_entries(input: PlanInput, opts: &PlanOptions) -> Result<SeedPlan, SeedError> {
    let globs = parse_globs(opts)?;
    let selection = select(&input.entries, &globs, &opts.exclude);
    let mut refusals = selection.refusals;
    refusals.extend(input.attrs.iter().map(|a| Refusal {
        path: a.path.clone(),
        kind: ProblemKind::CheckoutAttribute,
        detail: format!(
            "{}={} is set by the commit's own .gitattributes: a checkout's bytes would \
             differ from the commit's",
            a.attr, a.value
        ),
    }));
    let mut nodes = Vec::new();
    for (path, sha) in &selection.selected {
        if let Some(size) = input.sizes.get(sha).filter(|s| **s > read_limit(opts)) {
            refusals.push(refusal(
                path,
                ProblemKind::BodyTooLarge,
                &format!(
                    "the file is {size} bytes, more than the body limit ({}) plus {} bytes of \
                     header; it was not read",
                    opts.max_body_bytes,
                    super::HEADER_ALLOWANCE_BYTES
                ),
            ));
            continue;
        }
        let bytes = input
            .blobs
            .get(sha)
            .cloned()
            .ok_or_else(|| SeedError(format!("no bytes were read for {path} ({sha})")))?;
        match plan_file(path, sha, bytes, opts) {
            Ok(mut node) => {
                stamp(&mut node, &input, opts);
                nodes.push(node);
            }
            Err(problems) => refusals.extend(problems.into_iter().map(|p| at(path, p))),
        }
    }
    refusals.extend(duplicate_ids(&nodes));
    if opts.require_canonical {
        refusals.extend(nodes.iter().filter(|n| !n.canonical).map(|n| Refusal {
            path: n.path.clone(),
            kind: ProblemKind::NonCanonical,
            detail: "the header is not in canonical form (--require-canonical)".to_string(),
        }));
    }
    refusals.sort();
    let missing_code = match opts.code_property {
        Some(_) => nodes
            .iter()
            .filter(|n| n.code.is_none())
            .map(|n| n.path.clone())
            .collect(),
        None => Vec::new(),
    };
    Ok(SeedPlan {
        source: input.source,
        include: opts.effective_include(),
        max_body_bytes: opts.max_body_bytes,
        require_canonical: opts.require_canonical,
        code_property: opts.code_property.clone(),
        missing_code,
        history_blobs_read: input.history.blobs_read,
        nodes,
        excluded: selection.excluded,
        not_selected: selection.not_selected,
        refusals,
    })
}

/// Times and document code, which depend on the history and the run, not the file.
fn stamp(node: &mut PlannedNode, input: &PlanInput, opts: &PlanOptions) {
    let lineage = input.history.paths.get(&node.path);
    (node.modified_at, node.modified_source) = match lineage {
        Some(t) => (t.modified, TimeSource::GitPath),
        None => (input.received_at, TimeSource::Received),
    };
    (node.created_at, node.created_source) = match (input.history.ids.get(&node.file.id), lineage) {
        (Some(t), _) => (*t, TimeSource::GitId),
        (None, Some(t)) => (t.created, TimeSource::GitPath),
        (None, None) => (input.received_at, TimeSource::Received),
    };
    node.code = opts.code_property.as_ref().and_then(|key| {
        node.file
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    });
}

/// The largest selected file that is read at all.
fn read_limit(opts: &PlanOptions) -> u64 {
    (opts.max_body_bytes + super::HEADER_ALLOWANCE_BYTES) as u64
}

fn parse_globs(opts: &PlanOptions) -> Result<Vec<Glob>, SeedError> {
    opts.effective_include()
        .iter()
        .map(|p| Glob::new(p).map_err(|e| SeedError(e.0)))
        .collect()
}

fn at(path: &str, p: Problem) -> Refusal {
    Refusal {
        path: path.to_string(),
        kind: p.kind,
        detail: p.detail,
    }
}

/// Which tree entries are seeded, excluded, out of the selection, or refused.
#[derive(Default)]
struct Selection {
    /// `(path, blob sha)`, in path order.
    selected: Vec<(String, String)>,
    excluded: Vec<String>,
    not_selected: Vec<String>,
    refusals: Vec<Refusal>,
}

fn select(entries: &[TreeEntry], globs: &[Glob], exclude: &[String]) -> Selection {
    let mut sorted: Vec<&TreeEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| path_bytes(a).cmp(path_bytes(b)));
    let mut sel = Selection::default();
    let mut used = HashSet::new();
    for e in sorted {
        classify_entry(e, globs, exclude, &mut used, &mut sel);
    }
    for x in exclude.iter().filter(|x| !used.contains(*x)) {
        sel.refusals.push(Refusal {
            path: x.clone(),
            kind: ProblemKind::ExcludeNotSelected,
            detail: "--exclude names a path that is not in the selection".to_string(),
        });
    }
    if sel.selected.is_empty() && sel.refusals.is_empty() {
        sel.refusals.push(Refusal {
            path: String::new(),
            kind: ProblemKind::NothingSelected,
            detail: "no file in the tree matches --include".to_string(),
        });
    }
    sel
}

fn path_bytes(e: &TreeEntry) -> &[u8] {
    match &e.path {
        Ok(p) => p.as_bytes(),
        Err(raw) => raw,
    }
}

fn classify_entry(
    e: &TreeEntry,
    globs: &[Glob],
    exclude: &[String],
    used: &mut HashSet<String>,
    sel: &mut Selection,
) {
    let selected = |p: &str| globs.iter().any(|g| g.matches(p));
    let path = match &e.path {
        Ok(p) => p.clone(),
        Err(raw) => {
            let lossy = String::from_utf8_lossy(raw).into_owned();
            if selected(&lossy) || raw.ends_with(b".org") {
                sel.refusals.push(refusal(
                    &lossy,
                    ProblemKind::NonUtf8Path,
                    "the path is not UTF-8",
                ));
            } else {
                sel.not_selected.push(lossy);
            }
            return;
        }
    };
    if !selected(&path) {
        sel.not_selected.push(path);
    } else if exclude.contains(&path) {
        used.insert(path.clone());
        sel.excluded.push(path);
    } else if e.kind == "commit" {
        sel.refusals.push(refusal(
            &path,
            ProblemKind::Submodule,
            "a submodule, not a file",
        ));
    } else if e.mode == "120000" {
        sel.refusals.push(refusal(
            &path,
            ProblemKind::Symlink,
            "a symlink, not a file",
        ));
    } else if !path.ends_with(".org") {
        sel.refusals.push(refusal(
            &path,
            ProblemKind::NotOrgFile,
            "selected, but not an .org file",
        ));
    } else {
        sel.selected.push((path, e.sha.clone()));
    }
}

fn refusal(path: &str, kind: ProblemKind, detail: &str) -> Refusal {
    Refusal {
        path: path.to_string(),
        kind,
        detail: detail.to_string(),
    }
}

fn plan_file(
    path: &str,
    sha: &str,
    bytes: Vec<u8>,
    opts: &PlanOptions,
) -> Result<PlannedNode, Vec<Problem>> {
    let text = String::from_utf8(bytes).map_err(|_| {
        vec![Problem::new(
            ProblemKind::NotUtf8,
            "the file is not valid UTF-8",
        )]
    })?;
    let file = split(&text)?;
    let mut problems = Vec::new();
    if file.body.len() > opts.max_body_bytes {
        problems.push(Problem::new(
            ProblemKind::BodyTooLarge,
            format!(
                "the body is {} bytes; the limit is {} (it is never truncated)",
                file.body.len(),
                opts.max_body_bytes
            ),
        ));
    }
    let normalised = regen(&file);
    if !round_trips(&file, &normalised) {
        problems.push(Problem::new(
            ProblemKind::NotRoundTrippable,
            "the regenerated file does not split back to the same fields and body",
        ));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let canonical = normalised == text;
    Ok(PlannedNode {
        path: path.to_string(),
        blob: sha.to_string(),
        header_diff: (!canonical).then(|| line_diff(&text, &normalised)),
        file,
        canonical,
        created_at: 0,
        created_source: TimeSource::Received,
        modified_at: 0,
        modified_source: TimeSource::Received,
        code: None,
    })
}

/// G2: the regenerated file splits back to exactly these fields and body.
///
/// No input `split` accepts is known to fail this (the property tests generate
/// thousands and none does); it is kept because a failure would mean a seed
/// whose export does not read back as what was seeded, which no other check
/// would notice. Pinned by `plan_tests::the_round_trip_guard_rejects_a_file_that_does_not_read_back`.
pub(crate) fn round_trips(file: &SplitFile, normalised: &str) -> bool {
    matches!(split(normalised), Ok(ref again) if again == file)
}

/// Every id (file or anchor) that occurs in more than one file: one refusal
/// per occurrence, naming the other paths.
fn duplicate_ids(nodes: &[PlannedNode]) -> Vec<Refusal> {
    let mut seen: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for n in nodes {
        let ids = std::iter::once(&n.file.id).chain(&n.file.anchors);
        for id in ids.collect::<HashSet<_>>() {
            seen.entry(id).or_default().push(&n.path);
        }
    }
    let mut out = Vec::new();
    for (id, paths) in seen.into_iter().filter(|(_, p)| p.len() > 1) {
        for p in &paths {
            let others: Vec<&str> = paths.iter().copied().filter(|o| o != p).collect();
            out.push(refusal(
                p,
                ProblemKind::DuplicateId,
                &format!("the id {id:?} also appears in {}", others.join(", ")),
            ));
        }
    }
    out
}

/// The differing middle of two texts, by whole lines.
fn line_diff(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
    let la: Vec<&str> = a.split_inclusive('\n').collect();
    let lb: Vec<&str> = b.split_inclusive('\n').collect();
    let prefix = la.iter().zip(&lb).take_while(|(x, y)| x == y).count();
    let max_suffix = la.len().min(lb.len()) - prefix;
    let suffix = la
        .iter()
        .rev()
        .zip(lb.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    (
        own(&la[prefix..la.len() - suffix]),
        own(&lb[prefix..lb.len() - suffix]),
    )
}

fn plan_hash(nodes: &[PlannedNode]) -> String {
    let mut h = Sha256::new();
    field(&mut h, b"mae-seed-plan");
    field(&mut h, CONTRACT.as_bytes());
    count(&mut h, nodes.len());
    for n in nodes {
        let f = &n.file;
        field(&mut h, n.path.as_bytes());
        field(&mut h, f.id.as_bytes());
        h.update(n.created_at.to_le_bytes());
        h.update(n.modified_at.to_le_bytes());
        count(&mut h, f.properties.len());
        for (k, v) in &f.properties {
            field(&mut h, k.as_bytes());
            field(&mut h, v.as_bytes());
        }
        optional(&mut h, f.title.as_ref().map(|t| vec![t.clone()]));
        optional(&mut h, f.filetags.clone());
        field(&mut h, f.body.as_bytes());
    }
    hex::encode(h.finalize())
}

fn field(h: &mut Sha256, bytes: &[u8]) {
    count(h, bytes.len());
    h.update(bytes);
}

fn count(h: &mut Sha256, n: usize) {
    h.update((n as u64).to_le_bytes());
}

fn optional(h: &mut Sha256, v: Option<Vec<String>>) {
    match v {
        None => h.update([0u8]),
        Some(items) => {
            h.update([1u8]);
            count(h, items.len());
            for i in items {
                field(h, i.as_bytes());
            }
        }
    }
}

/// SHA-256 of `bytes`, hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
