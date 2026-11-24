//! `kb seed --update`: move a seeded KB to a newer plan of its repository.
//!
//! The KB keeps its id, collection (owner, members, policy, signed op-log),
//! identity bindings and every node document with its history. A node the new
//! plan changes is brought to it by [`KbNodeDoc::assign`], the field-by-field
//! CRDT diff (ADR-092), never replaced; a new node is written as the first
//! apply writes one; a node the plan no longer has is removed from the
//! collection's manifest, which is how a node is deleted today (its document is
//! kept, as `kb_delete_node` and `kb/collection_node_remove` keep it). A node
//! that comes back under the same `:ID:` is therefore the same document, with
//! its history. Before each overwrite and each removal the node's content is
//! cut as a version (ADR-116 D2.2: `seed`, `delete`).
//!
//! ## Edits outside the seed
//!
//! A node is "edited outside the seed" when its document's `version()` (a
//! digest of every materialized field) is not the version the seed left it at,
//! `seed_version` in its seed row. A row from before that column has none: the
//! node the seed wrote is then rebuilt from the row's blob in the repository
//! (`node_from_file`, the seed's one mapping), and compared field for field. An
//! edit is never inferred from a time. A node an earlier update removed and
//! this plan returns is judged against its version at the removal, and while
//! an update is in progress each node against that update's own row (see
//! `edits`). Any edit refuses the update, naming the node, the fields and the
//! editors, unless `--discard-edits` gives the dry run's `edits_hash` and the
//! edits still hash to it; the discarded content stays in the node's history.
//!
//! ## Atomicity
//!
//! As the first apply (`apply.rs`), ordered to converge:
//!
//! 1. Read-only checks: the plan has no refusal and is the reviewed one; the KB
//!    has a finished seed record, is a seed's literal, daemon-owned collection
//!    and is not end-to-end encrypted; no update to another plan is in
//!    progress; the plan reads the recorded repository; no node is edited
//!    outside the seed (or `--discard-edits` with the edits' hash).
//! 2. The update's intent, in ONE transaction (`storage_seed_update`): the new
//!    plan, and per node its new row, whether it is removed, and its version
//!    now. From here a re-run with the same plan resumes, a different plan is
//!    refused, and the rows are the node times.
//! 3. A resumed run checks again every node not yet done: it must still be at
//!    the version recorded in step 2, or already at the plan's content (or
//!    `--discard-edits` with the hash of those that are not).
//! 4. Each planned node, in path order, one write each: absent, written as a
//!    new document; different, assigned; equal, kept. Each is then marked done.
//!    New documents are not in the manifest yet, so no reader sees them.
//! 5. Each removed node's version is cut, and it is marked done with its
//!    document's version, which `kb_seed_removed` keeps once applied.
//! 6. The manifest: every new node added and every removed node removed in ONE
//!    update, so the KB's node set switches at once.
//! 7. The seed record takes the new plan and rows and is marked applied, in one
//!    transaction. A re-run after this writes nothing.
//!
//! A reader during steps 4 to 6 sees each node old or new (one CRDT update per
//! node, applied under its document's lock) and the old node set until step 6:
//! some nodes new, some old, never a node half of each.
//!
//! @ai-caution: [kb-truth] Never skip the edit check to make an update pass:
//! a node that differs from what the seed wrote is someone's content.
//! `--discard-edits` is the operator's decision, made after the dry run named
//! every such node, and bound by `edits_hash` to exactly those edits.

mod edits;

pub use edits::edits_hash;

use std::collections::{BTreeSet, HashMap};

use mae_sync::kb::history::{diff as field_diff, VersionKind};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, MaterializedNode, BODY_FORMAT_ORG_LITERAL_V1};

use super::apply::{
    build_node, checked_hash, collection_doc, existing_state, refuse, row_from_plan, storage,
    ApplyError, ApplyRefusalKind, ApplyTarget, BY,
};
use super::{PlannedNode, SeedPlan, CONTRACT};
use crate::doc_store::DocStore;
use crate::storage::{SeedNodeRow, SeedRecord, SeedUpdate, SeedUpdateNode};

/// What an update did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The KB is at the new plan. `created`: new documents; `written`: nodes
    /// brought to the plan now; `present`: nodes already at it; `removed`:
    /// nodes taken out of the manifest.
    Updated {
        created: usize,
        written: usize,
        present: usize,
        removed: usize,
    },
    /// The KB is already at this plan; nothing was written.
    AlreadyApplied,
}

/// One node the update adds, changes or removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeChange {
    pub node_id: String,
    /// The path in the new plan (for a removal, the recorded one).
    pub path: String,
    /// The recorded path, when the plan moves the node.
    pub from_path: Option<String>,
    /// The fields that change (`MaterializedNode` names). Empty for a removal.
    pub fields: Vec<&'static str>,
    /// An added node whose document already exists (it was removed by an
    /// earlier update): it returns with its history.
    pub returns: bool,
}

/// A node edited outside the seed since its last apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideEdit {
    pub node_id: String,
    pub path: String,
    /// The fields that differ from what the seed wrote, when that could be
    /// rebuilt; `None` when it could not (see `note`).
    pub fields: Option<Vec<&'static str>>,
    /// Who wrote the node since the seed's write (`kind:editor`), from the
    /// editor record.
    pub editors: Vec<String>,
    pub note: Option<String>,
    /// The document's `version()` now (`None`: no document), which the
    /// token covers.
    pub version: Option<String>,
    /// The apply would discard this edit (overwrite or remove the node).
    /// `false` only for a node an update in progress already wrote: the
    /// resume keeps that edit, and the next update finds it.
    pub discarded: bool,
}

/// The dry run's comparison of a plan with the KB as applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDiff {
    pub kb_id: String,
    /// The record of the last applied plan.
    pub applied: SeedRecord,
    /// The plan of an update in progress, if one was cut short.
    pub in_progress: Option<String>,
    pub added: Vec<NodeChange>,
    pub changed: Vec<NodeChange>,
    pub removed: Vec<NodeChange>,
    pub unchanged: usize,
    pub edited: Vec<OutsideEdit>,
    /// [`edits_hash`] of `edited`: what `--discard-edits` must give. `None`
    /// when the apply would discard no edit.
    pub edits_hash: Option<String>,
}

/// The KB as the last apply left it.
struct Applied {
    record: SeedRecord,
    rows: HashMap<String, SeedNodeRow>,
}

/// A planned node against the KB: its row once updated, its document now,
/// and the node the plan makes of it.
struct Placed {
    row: SeedNodeRow,
    doc: Option<KbNodeDoc>,
    target: KbNodeDoc,
}

/// Compare `plan` with KB `kb_id` as applied: the dry run of an update.
pub async fn diff(
    store: &DocStore,
    plan: &SeedPlan,
    kb_id: &str,
    owner_fp: &str,
) -> Result<UpdateDiff, ApplyError> {
    let applied = load_applied(store, kb_id, owner_fp).await?;
    check_repo(plan, &applied.record)?;
    let progress = store.seed_update(kb_id).await.map_err(storage)?;
    // Mid-update, each node is judged against the update's own row: the
    // applied rows no longer describe the nodes it has written.
    let edited = match &progress {
        Some((u, nodes)) => {
            let same = plan.plan_hash().is_some_and(|h| h == u.plan_hash);
            edits::in_progress(store, kb_id, same.then_some(plan), nodes).await?
        }
        None => edits::fresh(store, kb_id, plan, &applied.rows).await?,
    };
    let placed = place(store, kb_id, plan, &applied.rows).await?;
    let mut d = UpdateDiff {
        kb_id: kb_id.to_string(),
        applied: applied.record.clone(),
        in_progress: progress.map(|(u, _)| u.plan_hash),
        added: Vec::new(),
        changed: Vec::new(),
        removed: removed_rows(plan, &applied.rows).map(removal).collect(),
        unchanged: 0,
        edits_hash: edits_hash(&edited),
        edited,
    };
    for p in &placed {
        classify(p, applied.rows.get(&p.row.node_id), &mut d);
    }
    Ok(d)
}

fn removal(r: &SeedNodeRow) -> NodeChange {
    NodeChange {
        node_id: r.node_id.clone(),
        path: r.path.clone(),
        from_path: None,
        fields: Vec::new(),
        returns: false,
    }
}

fn classify(p: &Placed, old: Option<&SeedNodeRow>, d: &mut UpdateDiff) {
    let mut change = NodeChange {
        node_id: p.row.node_id.clone(),
        path: p.row.path.clone(),
        from_path: old.filter(|o| o.path != p.row.path).map(|o| o.path.clone()),
        fields: Vec::new(),
        returns: false,
    };
    let Some(doc) = &p.doc else {
        d.added.push(change);
        return;
    };
    change.fields = changed_fields(&doc.materialize(), &p.target.materialize());
    if old.is_none() {
        change.returns = true;
        d.added.push(change);
    } else if change.fields.is_empty() && change.from_path.is_none() {
        d.unchanged += 1;
    } else {
        d.changed.push(change);
    }
}

/// The fields a write would change, `created_at` aside (no write moves it).
fn changed_fields(current: &MaterializedNode, target: &MaterializedNode) -> Vec<&'static str> {
    field_diff(current, target)
        .into_iter()
        .map(|c| c.field)
        .filter(|f| *f != "created_at")
        .collect()
}

/// Update KB `t.kb_id` to `plan`. `discard_edits`: the dry run's
/// `edits_hash`, to overwrite the edits it named. See the module doc for the
/// order.
pub async fn update(
    store: &DocStore,
    plan: &SeedPlan,
    t: &ApplyTarget<'_>,
    discard_edits: Option<&str>,
) -> Result<UpdateOutcome, ApplyError> {
    let hash = checked_hash(plan, t.plan_hash)?;
    let applied = load_applied(store, t.kb_id, &t.owner.fingerprint()).await?;
    check_repo(plan, &applied.record)?;
    let nodes = match store.seed_update(t.kb_id).await.map_err(storage)? {
        Some((u, _)) if u.plan_hash != hash => {
            return Err(refuse(
                ApplyRefusalKind::DifferentPlan,
                format!(
                    "an update of KB '{}' to plan {} (rev {}) is in progress; finish it with \
                     that plan first",
                    t.kb_id, u.plan_hash, u.rev
                ),
            ))
        }
        Some((_, nodes)) => resumable(store, t.kb_id, plan, nodes, discard_edits).await?,
        None if applied.record.plan_hash == hash => return Ok(UpdateOutcome::AlreadyApplied),
        None => begin(store, plan, &hash, &applied, t, discard_edits).await?,
    };
    crash::at(crash::Step::Begun)?;
    write(store, t, plan, &nodes).await
}

/// The record of a finished seed of a literal, daemon-owned, readable KB.
async fn load_applied(
    store: &DocStore,
    kb_id: &str,
    owner_fp: &str,
) -> Result<Applied, ApplyError> {
    let Some((record, rows)) = store.seed_record(kb_id).await.map_err(storage)? else {
        return Err(refuse(
            ApplyRefusalKind::NotSeeded,
            format!("KB '{kb_id}' has no seed record: only a KB made by `kb seed` can be updated"),
        ));
    };
    if record.applied_at.is_none() {
        return Err(refuse(
            ApplyRefusalKind::SeedUnfinished,
            format!(
                "the seed of KB '{kb_id}' (plan {}) never finished: re-run `kb seed` without \
                 --update and with that plan first",
                record.plan_hash
            ),
        ));
    }
    if record.contract != CONTRACT {
        return Err(refuse(
            ApplyRefusalKind::DifferentPlan,
            format!(
                "KB '{kb_id}' was seeded under contract {}, not {CONTRACT}",
                record.contract
            ),
        ));
    }
    check_collection(store, kb_id, owner_fp).await?;
    let rows = rows.into_iter().map(|r| (r.node_id.clone(), r)).collect();
    Ok(Applied { record, rows })
}

/// The plan must read the repository the KB was seeded from, compared as
/// recorded: the path as the operator gave it.
/// The plan must come from the same history the KB was seeded from: the
/// commit last applied is the new revision or in its history, in the
/// repository the plan was read from. A path is not an identity: a
/// repository moved on disk, or given by a relative path from another
/// directory, is the same history and is accepted (the record then takes
/// the new path); an unrelated repository, or a history rewritten past the
/// applied commit, is refused. Its recourse is a new kb-id.
fn check_repo(plan: &SeedPlan, record: &SeedRecord) -> Result<(), ApplyError> {
    if plan.source.repo == record.repo && plan.source.commit == record.commit {
        return Ok(());
    }
    let repo = super::git::GitRepo::new(&plan.source.repo);
    let refused = |why: String| {
        Err(refuse(
            ApplyRefusalKind::DifferentRepo,
            format!(
                "KB '{}' was seeded from commit {} ({:?}); {why}. A rewritten or unrelated \
                 history needs a new kb-id",
                record.kb_id, record.commit, record.repo
            ),
        ))
    };
    match repo.is_ancestor(&record.commit, &plan.source.commit) {
        Ok(true) => Ok(()),
        Ok(false) => refused(format!(
            "it is not in the history of {} in {:?}",
            plan.source.commit, plan.source.repo
        )),
        Err(e) => refused(format!("{:?} does not hold it: {e}", plan.source.repo)),
    }
}

async fn check_collection(store: &DocStore, kb_id: &str, owner_fp: &str) -> Result<(), ApplyError> {
    let differs = |why: &str| {
        refuse(
            ApplyRefusalKind::CollectionDiffers,
            format!("KB '{kb_id}' is not a seed's collection: {why}"),
        )
    };
    let Some(bytes) = existing_state(store, &collection_doc(kb_id)).await? else {
        return Err(differs("it has no collection"));
    };
    let coll = KbCollectionDoc::from_bytes(&bytes).map_err(|_| differs("it does not decode"))?;
    if crate::collab_handler::kb_is_e2e(store, kb_id, &coll).await {
        return Err(refuse(
            ApplyRefusalKind::Encrypted,
            format!("KB '{kb_id}' is end-to-end encrypted: the daemon cannot update its content"),
        ));
    }
    if coll.body_format().as_deref() != Some(BODY_FORMAT_ORG_LITERAL_V1) {
        return Err(differs("its body format is not org-literal-v1"));
    }
    if coll.owner() != owner_fp {
        return Err(differs("this daemon's identity does not own it"));
    }
    Ok(())
}

/// Each planned node against the KB.
async fn place(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<Vec<Placed>, ApplyError> {
    let mut out = Vec::with_capacity(plan.nodes.len());
    for n in &plan.nodes {
        let doc = read_node(store, kb_id, &n.file.id).await?;
        let row = new_row(n, rows.get(&n.file.id), doc.as_ref());
        let target = build_node(n, &row);
        out.push(Placed { row, doc, target });
    }
    Ok(out)
}

/// A node's document, `None` if it has none. One that does not decode is an
/// error: it is never taken for absent.
async fn read_node(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
) -> Result<Option<KbNodeDoc>, ApplyError> {
    let name = mae_sync::kb_node_doc_name(kb_id, node_id);
    let Some(bytes) = existing_state(store, &name).await? else {
        return Ok(None);
    };
    KbNodeDoc::from_bytes(&bytes).map(Some).map_err(|e| {
        refuse(
            ApplyRefusalKind::NodeDiffers,
            format!("node '{node_id}' does not decode as a node: {e}"),
        )
    })
}

/// The row a planned node gets. Its creation time is the document's when it
/// has one (no write moves it), so the row and the node agree.
fn new_row(n: &PlannedNode, old: Option<&SeedNodeRow>, doc: Option<&KbNodeDoc>) -> SeedNodeRow {
    let mut row = row_from_plan(n);
    if let Some(o) = old {
        row.created_at = o.created_at;
        row.created_source = o.created_source.clone();
    } else if let Some(c) = doc.and_then(KbNodeDoc::created_at) {
        row.created_at = c;
    }
    row
}

fn removed_rows<'a>(
    plan: &SeedPlan,
    rows: &'a HashMap<String, SeedNodeRow>,
) -> impl Iterator<Item = &'a SeedNodeRow> {
    let planned: BTreeSet<&str> = plan.nodes.iter().map(|n| n.file.id.as_str()).collect();
    let mut removed: Vec<&SeedNodeRow> = rows
        .values()
        .filter(|r| !planned.contains(r.node_id.as_str()))
        .collect();
    removed.sort_by(|a, b| a.path.cmp(&b.path));
    removed.into_iter()
}

/// A fresh update: check, then record its intent.
async fn begin(
    store: &DocStore,
    plan: &SeedPlan,
    hash: &str,
    applied: &Applied,
    t: &ApplyTarget<'_>,
    discard_edits: Option<&str>,
) -> Result<Vec<SeedUpdateNode>, ApplyError> {
    // @ai-caution: [seed-update] The edit check, `place` and the writes are
    // separate reads of the documents. Nothing can write in between only
    // because the whole update runs under `open_exclusive` (the store lock,
    // and no daemon serving): an edit that landed after this check would be
    // taken as the base and overwritten unreviewed. Keep the update an
    // operator command under that lock, or carry the checked versions into
    // `place` and compare them at the write.
    let edited = edits::fresh(store, t.kb_id, plan, &applied.rows).await?;
    edits::check_discard(&edited, discard_edits)?;
    let mut nodes = Vec::new();
    for p in place(store, t.kb_id, plan, &applied.rows).await? {
        nodes.push(SeedUpdateNode {
            row: p.row,
            remove: false,
            base_version: p.doc.as_ref().map(KbNodeDoc::version),
            done: false,
        });
    }
    for r in removed_rows(plan, &applied.rows) {
        let doc = read_node(store, t.kb_id, &r.node_id).await?;
        nodes.push(SeedUpdateNode {
            row: r.clone(),
            remove: true,
            base_version: doc.as_ref().map(KbNodeDoc::version),
            done: false,
        });
    }
    let intent = SeedUpdate {
        kb_id: t.kb_id.to_string(),
        plan_hash: hash.to_string(),
        repo: plan.source.repo.clone(),
        rev: plan.source.rev.clone(),
        commit: plan.source.commit.clone(),
        tree: plan.source.tree.clone(),
        excluded: plan.excluded.clone(),
        started_at: t.now,
    };
    store
        .seed_update_begin(&intent, &nodes)
        .await
        .map_err(storage)?;
    Ok(nodes)
}

/// The recorded nodes of an update cut short: they must name this plan's
/// nodes, and every node not yet done must be where the update left it (its
/// version when the update began) or already at the plan, unless
/// `discard_edits` is the hash of those that are not.
async fn resumable(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    nodes: Vec<SeedUpdateNode>,
    discard_edits: Option<&str>,
) -> Result<Vec<SeedUpdateNode>, ApplyError> {
    let planned: BTreeSet<&str> = plan.nodes.iter().map(|n| n.file.id.as_str()).collect();
    let recorded: BTreeSet<&str> = nodes
        .iter()
        .filter(|n| !n.remove)
        .map(|n| n.row.node_id.as_str())
        .collect();
    if planned != recorded {
        return Err(refuse(
            ApplyRefusalKind::DifferentPlan,
            "the update's recorded nodes are not this plan's, though the hashes agree: the \
             record is damaged",
        ));
    }
    let moved = edits::in_progress(store, kb_id, Some(plan), &nodes).await?;
    edits::check_discard(&moved, discard_edits)?;
    Ok(nodes)
}

/// The document already has the plan's content (`created_at` aside).
fn same_content(doc: &KbNodeDoc, target: &KbNodeDoc) -> bool {
    changed_fields(&doc.materialize(), &target.materialize()).is_empty()
}

/// Steps 4 to 7.
async fn write(
    store: &DocStore,
    t: &ApplyTarget<'_>,
    plan: &SeedPlan,
    nodes: &[SeedUpdateNode],
) -> Result<UpdateOutcome, ApplyError> {
    let by_id: HashMap<&str, &SeedUpdateNode> = nodes
        .iter()
        .filter(|n| !n.remove)
        .map(|n| (n.row.node_id.as_str(), n))
        .collect();
    let mut count = Counts::default();
    for (i, p) in plan.nodes.iter().enumerate() {
        crash::at(crash::Step::BeforeNode(i))?;
        let u = by_id[p.file.id.as_str()];
        if !u.done {
            write_node(store, t, p, u, i, &mut count).await?;
        }
    }
    for u in nodes.iter().filter(|n| n.remove && !n.done) {
        remove_node(store, t, u).await?;
        count.removed += 1;
    }
    crash::at(crash::Step::BeforeManifest)?;
    write_manifest(store, t.kb_id, plan, nodes).await?;
    crash::at(crash::Step::BeforeFinish)?;
    store
        .seed_update_finish(t.kb_id, t.now)
        .await
        .map_err(storage)?;
    Ok(UpdateOutcome::Updated {
        created: count.created,
        written: count.written,
        present: count.present,
        removed: count.removed,
    })
}

#[derive(Default)]
struct Counts {
    created: usize,
    written: usize,
    present: usize,
    removed: usize,
}

/// Bring one planned node to the plan and mark it done.
async fn write_node(
    store: &DocStore,
    t: &ApplyTarget<'_>,
    p: &PlannedNode,
    u: &SeedUpdateNode,
    i: usize,
    count: &mut Counts,
) -> Result<(), ApplyError> {
    let name = mae_sync::kb_node_doc_name(t.kb_id, &p.file.id);
    let target = build_node(p, &u.row);
    let (wal, version) = match read_node(store, t.kb_id, &p.file.id).await? {
        None => {
            count.created += 1;
            let r = store.restore_doc(&name, &target.encode(), BY).await;
            (r.map_err(storage)?.wal_seq, target.version())
        }
        Some(doc) if same_content(&doc, &target) => {
            count.present += 1;
            let wal = store.wal_high_water(&name).await.map_err(storage)?;
            (wal, doc.version())
        }
        Some(mut doc) => {
            count.written += 1;
            cut(store, t.kb_id, &p.file.id, VersionKind::Seed).await?;
            let update = doc.assign(&target.materialize());
            let r = store.apply_update(&name, &update, None, BY).await;
            (r.map_err(storage)?.wal_seq, doc.version())
        }
    };
    crash::at(crash::Step::AfterWrite(i))?;
    store
        .seed_update_node_done(t.kb_id, &p.file.id, Some(wal), Some(&version))
        .await
        .map_err(storage)
}

/// ADR-116 D2.2: the node's content, kept as a version before it is replaced
/// or removed. A version equal to the last one is not written again.
async fn cut(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
    kind: VersionKind,
) -> Result<(), ApplyError> {
    store
        .history_checkpoint(kb_id, node_id, kind, None, None)
        .await
        .map(|_| ())
        .map_err(storage)
}

/// Step 5 for one removed node: its version cut, then done with the version
/// its document is left at, against which a later return is judged. The
/// manifest change is step 6's.
async fn remove_node(
    store: &DocStore,
    t: &ApplyTarget<'_>,
    u: &SeedUpdateNode,
) -> Result<(), ApplyError> {
    let id = &u.row.node_id;
    let (wal, version) = match read_node(store, t.kb_id, id).await? {
        Some(doc) => {
            cut(store, t.kb_id, id, VersionKind::Delete).await?;
            let name = mae_sync::kb_node_doc_name(t.kb_id, id);
            let wal = store.wal_high_water(&name).await.map_err(storage)?;
            (Some(wal), Some(doc.version()))
        }
        None => (None, None),
    };
    store
        .seed_update_node_done(t.kb_id, id, wal, version.as_deref())
        .await
        .map_err(storage)
}

/// Step 6: every new node in the manifest and every removed one out of it, as
/// one update. Nothing is written when the manifest is already so.
async fn write_manifest(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    nodes: &[SeedUpdateNode],
) -> Result<(), ApplyError> {
    let name = collection_doc(kb_id);
    let bytes = store.encode_state(&name).await.map_err(storage)?;
    let mut coll = KbCollectionDoc::from_bytes(&bytes).map_err(|e| {
        refuse(
            ApplyRefusalKind::CollectionDiffers,
            format!("KB '{kb_id}': the collection does not decode: {e}"),
        )
    })?;
    // Whether anything changed is counted, not read off the state vector: a
    // removal is a deletion, which does not advance it.
    let before = coll.state_vector();
    let mut changed = false;
    for n in &plan.nodes {
        if !coll.has_node(&n.file.id) {
            let _ = coll.add_node(&n.file.id, n.file.title.as_deref().unwrap_or(""));
            changed = true;
        }
    }
    for u in nodes.iter().filter(|n| n.remove) {
        if coll.has_node(&u.row.node_id) {
            let _ = coll.remove_node(&u.row.node_id);
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let update = coll.encode_diff(&before).map_err(|e| {
        refuse(
            ApplyRefusalKind::Storage,
            format!("KB '{kb_id}': manifest update: {e}"),
        )
    })?;
    store
        .apply_update(&name, &update, None, BY)
        .await
        .map_err(storage)?;
    Ok(())
}

/// Test-only crash points: stop the update where a killed process would.
/// Everything written before it is durable. Compiled to nothing outside tests.
pub(crate) mod crash {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Step {
        /// After the intent is recorded, before any node.
        Begun,
        /// Before planned node `i`.
        BeforeNode(usize),
        /// After node `i`'s document is written, before it is marked done.
        AfterWrite(usize),
        /// After every node, before the manifest.
        BeforeManifest,
        /// After the manifest, before the record is marked applied.
        BeforeFinish,
    }

    #[cfg(test)]
    thread_local! {
        pub(crate) static AT: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
    }

    #[cfg(test)]
    pub(super) fn at(step: Step) -> Result<(), super::ApplyError> {
        match AT.with(std::cell::Cell::get) {
            Some(s) if s == step => Err(super::refuse(
                super::ApplyRefusalKind::Storage,
                format!("injected crash at {step:?}"),
            )),
            _ => Ok(()),
        }
    }

    #[cfg(not(test))]
    #[inline(always)]
    pub(super) fn at(_step: Step) -> Result<(), super::ApplyError> {
        Ok(())
    }
}
