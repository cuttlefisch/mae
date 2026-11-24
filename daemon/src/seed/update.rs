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
//! edit is never inferred from a time. Any edit refuses the update, naming the
//! node, the fields and the editors, unless `--discard-edits`; the discarded
//! content stays in the node's history.
//!
//! ## Atomicity
//!
//! As the first apply (`apply.rs`), ordered to converge:
//!
//! 1. Read-only checks: the plan has no refusal and is the reviewed one; the KB
//!    has a finished seed record, is a seed's literal, daemon-owned collection
//!    and is not end-to-end encrypted; no update to another plan is in
//!    progress; no node is edited outside the seed (or `--discard-edits`).
//! 2. The update's intent, in ONE transaction (`storage_seed_update`): the new
//!    plan, and per node its new row, whether it is removed, and its version
//!    now. From here a re-run with the same plan resumes, a different plan is
//!    refused, and the rows are the node times.
//! 3. A resumed run checks again every node not yet done: it must still be at
//!    the version recorded in step 2, or already at the plan's content.
//! 4. Each planned node, in path order, one write each: absent, written as a
//!    new document; different, assigned; equal, kept. Each is then marked done.
//!    New documents are not in the manifest yet, so no reader sees them.
//! 5. Each removed node's version is cut, and it is marked done.
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
//! every such node.

use std::collections::{BTreeSet, HashMap};

use mae_sync::kb::history::{diff as field_diff, VersionKind};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, MaterializedNode, BODY_FORMAT_ORG_LITERAL_V1};

use super::apply::{
    build_node, checked_hash, collection_doc, existing_state, refuse, row_from_plan, storage,
    ApplyError, ApplyRefusalKind, ApplyTarget, BY,
};
use super::{node_from_file, split, GitRepo, PlannedNode, SeedPlan, CONTRACT};
use crate::doc_editors::{EditorKind, EditorsSince, InternalWriter};
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
    let in_progress = store
        .seed_update(kb_id)
        .await
        .map_err(storage)?
        .map(|(u, _)| u.plan_hash);
    let placed = place(store, kb_id, plan, &applied.rows).await?;
    let mut d = UpdateDiff {
        kb_id: kb_id.to_string(),
        applied: applied.record.clone(),
        in_progress,
        added: Vec::new(),
        changed: Vec::new(),
        removed: removed_rows(plan, &applied.rows).map(removal).collect(),
        unchanged: 0,
        edited: outside_edits(store, kb_id, &plan.source.repo, &applied.rows).await?,
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

/// Update KB `t.kb_id` to `plan`. See the module doc for the order.
pub async fn update(
    store: &DocStore,
    plan: &SeedPlan,
    t: &ApplyTarget<'_>,
    discard_edits: bool,
) -> Result<UpdateOutcome, ApplyError> {
    let hash = checked_hash(plan, t.plan_hash)?;
    let applied = load_applied(store, t.kb_id, &t.owner.fingerprint()).await?;
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
    discard_edits: bool,
) -> Result<Vec<SeedUpdateNode>, ApplyError> {
    let edited = outside_edits(store, t.kb_id, &plan.source.repo, &applied.rows).await?;
    if !edited.is_empty() && !discard_edits {
        return Err(edited_refusal(&edited));
    }
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

fn edited_refusal(edited: &[OutsideEdit]) -> ApplyError {
    let names: Vec<String> = edited
        .iter()
        .map(|e| format!("{} ({})", e.node_id, e.path))
        .collect();
    refuse(
        ApplyRefusalKind::EditedOutsideSeed,
        format!(
            "{} node(s) were edited outside the seed since its last apply: {}; run the dry \
             run to see how, and give --discard-edits to overwrite them (the edits stay in \
             each node's history)",
            edited.len(),
            names.join(", ")
        ),
    )
}

/// The recorded nodes of an update cut short: they must name this plan's
/// nodes, and every node not yet done must be where the update left it (its
/// version when the update began) or already at the plan.
async fn resumable(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    nodes: Vec<SeedUpdateNode>,
    discard_edits: bool,
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
    let by_id: HashMap<&str, &PlannedNode> =
        plan.nodes.iter().map(|n| (n.file.id.as_str(), n)).collect();
    let mut moved = Vec::new();
    for n in nodes.iter().filter(|n| !n.done) {
        let doc = read_node(store, kb_id, &n.row.node_id).await?;
        let now = doc.as_ref().map(KbNodeDoc::version);
        let at_plan = match (by_id.get(n.row.node_id.as_str()), &doc) {
            (Some(p), Some(d)) => same_content(d, &build_node(p, &n.row)),
            _ => false,
        };
        if now != n.base_version && !at_plan {
            moved.push(OutsideEdit {
                node_id: n.row.node_id.clone(),
                path: n.row.path.clone(),
                fields: None,
                editors: Vec::new(),
                note: Some("written since this update began".into()),
            });
        }
    }
    if !moved.is_empty() && !discard_edits {
        return Err(edited_refusal(&moved));
    }
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

/// Step 5 for one removed node: its version, then done. The manifest change
/// is step 6's.
async fn remove_node(
    store: &DocStore,
    t: &ApplyTarget<'_>,
    u: &SeedUpdateNode,
) -> Result<(), ApplyError> {
    if read_node(store, t.kb_id, &u.row.node_id).await?.is_some() {
        cut(store, t.kb_id, &u.row.node_id, VersionKind::Delete).await?;
    }
    store
        .seed_update_node_done(t.kb_id, &u.row.node_id, None, None)
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

/// Every recorded node whose document is not what the seed last wrote.
async fn outside_edits(
    store: &DocStore,
    kb_id: &str,
    repo: &str,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<Vec<OutsideEdit>, ApplyError> {
    let mut rows: Vec<&SeedNodeRow> = rows.values().collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut docs = HashMap::new();
    for r in &rows {
        docs.insert(
            r.node_id.clone(),
            read_node(store, kb_id, &r.node_id).await?,
        );
    }
    let suspects: Vec<&SeedNodeRow> = rows
        .iter()
        .copied()
        .filter(|r| {
            let now = docs[&r.node_id].as_ref().map(KbNodeDoc::version);
            r.seed_version.is_none() || now != r.seed_version
        })
        .collect();
    let written = seed_written(repo, &suspects);
    let mut out = Vec::new();
    for r in suspects {
        let doc = docs[&r.node_id].as_ref();
        if let Some(e) = judge(r, doc, written.get(&r.node_id)) {
            out.push(with_editors(store, kb_id, r, e).await);
        }
    }
    Ok(out)
}

/// Is `r`'s node edited? `written`: the node the seed wrote, rebuilt from the
/// recorded blob (or why it could not be).
fn judge(
    r: &SeedNodeRow,
    doc: Option<&KbNodeDoc>,
    written: Option<&Result<KbNodeDoc, String>>,
) -> Option<OutsideEdit> {
    let mut e = OutsideEdit {
        node_id: r.node_id.clone(),
        path: r.path.clone(),
        fields: None,
        editors: Vec::new(),
        note: None,
    };
    let Some(doc) = doc else {
        e.note = Some("the node's document is missing".into());
        return Some(e);
    };
    let w = match written {
        Some(Ok(w)) => w,
        Some(Err(why)) => {
            e.note = Some(match r.seed_version {
                None => format!(
                    "cannot tell: the seed record predates its version digest and the \
                     seed's write cannot be rebuilt ({why})"
                ),
                Some(_) => format!("differs from the seed's write; fields unknown ({why})"),
            });
            return Some(e);
        }
        None => {
            e.note = Some("differs from the seed's write".into());
            return Some(e);
        }
    };
    if r.seed_version.as_ref().is_some_and(|v| *v != w.version()) {
        e.note = Some(
            "differs from the seed's write; fields unknown (the recorded blob does not \
             rebuild it)"
                .into(),
        );
        return Some(e);
    }
    let fields = field_diff(&w.materialize(), &doc.materialize());
    if fields.is_empty() {
        return None;
    }
    e.fields = Some(fields.into_iter().map(|c| c.field).collect());
    Some(e)
}

/// Who wrote the node since the seed's write, from the editor record.
async fn with_editors(
    store: &DocStore,
    kb_id: &str,
    r: &SeedNodeRow,
    mut e: OutsideEdit,
) -> OutsideEdit {
    let name = mae_sync::kb_node_doc_name(kb_id, &r.node_id);
    let seed = InternalWriter::Seed.as_str();
    match store
        .editors_since_wal(&name, r.seed_wal_id.unwrap_or(0))
        .await
    {
        Ok(EditorsSince::Known(rows)) => {
            e.editors = rows
                .into_iter()
                .filter(|w| !(w.kind == EditorKind::Internal && w.editor == seed))
                .map(|w| format!("{}:{}", w.kind.as_str(), w.editor))
                .collect();
        }
        Ok(_) => e.editors = vec!["(the edit record does not cover it)".into()],
        Err(err) => e.editors = vec![format!("(edit record unreadable: {err})")],
    }
    e
}

/// The node the seed wrote for each row, rebuilt from its blob: the seed's one
/// mapping (`node_from_file`) of the file, with the recorded creation time.
/// Every blob is hash-checked; one git process reads them all.
fn seed_written(repo: &str, rows: &[&SeedNodeRow]) -> HashMap<String, Result<KbNodeDoc, String>> {
    if rows.is_empty() {
        return HashMap::new();
    }
    let git = GitRepo::new(repo);
    let shas: Vec<String> = rows.iter().map(|r| r.blob.clone()).collect();
    let read = git
        .object_format()
        .and_then(|f| git.read_blobs(&shas).map(|b| (f, b)));
    let (format, blobs) = match read {
        Ok(v) => v,
        Err(e) => {
            return rows
                .iter()
                .map(|r| (r.node_id.clone(), Err(e.to_string())))
                .collect()
        }
    };
    rows.iter()
        .zip(blobs)
        .map(|(r, bytes)| (r.node_id.clone(), rebuild(r, &format, &bytes)))
        .collect()
}

fn rebuild(r: &SeedNodeRow, format: &str, bytes: &[u8]) -> Result<KbNodeDoc, String> {
    if super::git::blob_name(format, bytes) != r.blob {
        return Err(format!("blob {} does not hash to its name", r.blob));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the blob is not UTF-8".to_string())?;
    let file = split(text).map_err(|_| "the blob no longer splits".to_string())?;
    Ok(node_from_file(&file, r.created_at))
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
